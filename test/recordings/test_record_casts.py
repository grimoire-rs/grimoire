# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The single parametrized runner for every cast under
``test/recordings/casts/*.yaml`` (design contract C-029, plan decision
D-8). Replaces the hardcoded ``test_record_demo.py``.

Not part of `task verify` -- this file sits outside the acceptance suite's
`testpaths = ["tests"]` (`test/pyproject.toml`), so a bare `pytest`/`task
test` never collects it. Run it explicitly via `task test:demo` (see
`test/taskfile.yml`).

**The whole point of recording inside an assertion**: a cast is produced
only when the commands actually succeeded and their output matched. A step
that asserts nothing cannot ship, which is why a step without `expect` is a
*usage error* that fails collection.

**No new Python dependency** (`test/pyproject.toml` dev group is pytest,
pytest-xdist, pexpect, rich -- and it is not in this work package's file
set). The YAML reader below therefore supports a deliberately small subset
and rejects everything outside it loudly, rather than mis-parsing a page
author's file silently.

The supported YAML subset, stated exactly (this is the contract eleven
later casts are written against):

- `#` opens a comment on its own line, and ` #` opens one at the end of a
  plain scalar -- YAML's own rule, followed exactly so this subset never
  disagrees silently with another reader. A value that must contain ` #`
  is quoted; the comment then starts after the closing quote. Blank lines
  ignored.
- Top-level `key: value` scalars. A bare digit-run becomes an `int`;
  anything else is a string. A value may be wrapped in matching single or
  double quotes, which are stripped -- needed when the value contains ": ".
- `steps:` introduces a block sequence of mappings. Each item starts
  `  - <key>: <value>` and continues with more-indented `    <key>: <value>`
  lines.
- `dirs:` introduces a block sequence of plain scalars: `  - .claude` on
  its own line, one per entry.
- Anything else -- block scalars (`|`, `>`), nested mappings, flow
  collections (`[...]`, `{...}`), anchors, multiple documents -- is a usage
  error naming the file and the 1-based line number.

The key set: `output` (required, path relative to the repo root), `title`
(optional, the asciicast header title), `registry` (`local` or `public`,
**default `local`** -- C-029 says every cast but one records against the
session fixture, so omitting the key must not silently reach GHCR),
`columns` / `rows` (optional PTY size, defaults 180 / 24 from
`CastRecorder`), `dirs` (optional, directories created in the recording
workspace before the shell opens), `steps` (required, non-empty), `keys`
(optional, see below), `raw` (optional, see below), `seed` (optional, see
below).

`keys:` is a block sequence, shaped exactly like `steps:` items: each entry
is a flat mapping carrying `send` (required, the raw key sequence to write
to the PTY) and `wait` (optional, seconds to drain output for afterward,
default `0.5`). It exists for driving a full-screen application (`grim
tui`) key by key rather than a line at a time. `send`'s value is read
verbatim except for one escape: the two-character sequences `\\e`, `\\n`
and `\\t` become `\\x1b`, a real newline and a real tab -- the one place a
cast YAML performs any escape processing at all.

`raw: true` marks the **last** step as launched in raw-stream mode: the
`keys:` schedule is sent to it via `CastRecorder.run_raw`, and that step's
`expect` is matched against the escape-stripped raw stream instead of a
line-buffered command's output. Every earlier step still runs through the
normal `run_command` path. Only the literal strings `true` and `false` are
accepted. A script declaring `keys:` must declare `raw: true` -- a `keys:`
schedule with nowhere to send it is a usage error.

`seed:` is a block sequence of plain scalars, the same shape as `dirs:`.
Each entry is an OCI repository path (e.g. `grim-docs/hello-world`)
published into the session's local `registry:2` fixture before the shell
opens, so a `registry: local` cast has something to `grim add`. It exists
because the parametrized runner starts against an empty registry, and the
alternative -- visible `grim release` steps recorded into the cast itself
-- would put registry-publishing noise into a first-steps screencast.
`seed:` on a `registry: public` cast is a usage error: there is no session
registry to push a seed artifact into.

Every `expect` is a regex that must match something the command actually
printed. A pattern matching the empty string (`.*`, `^`, `x?`) is rejected
at load time: it would record a cast over zero output, which is the one
thing this harness exists to prevent.

`{registry}` inside a step's `type` is replaced with the session registry
host, and only a `registry: local` cast may use it. Every `registry: local`
cast is guarded: its built cast must contain no `ghcr.io` reference.

**Never transcribe a digest or a hash out of a cast into a documentation
page.** The session registry binds to a random port, grim bakes the resulting
reference into what it hashes, so every manifest digest and every
`declaration_hash` a `registry: local` cast prints changes on the next
recording -- and is shown on the page against a `ghcr.io/...` reference it
never belonged to, which no reader can reproduce. Let the embedded recording
carry those values and describe the column in prose instead.

`demo.yaml` is the one `registry: public` cast -- design record § 8,
amendment E-8.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import uuid
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.helpers import make_artifact
from src.runner import GrimRunner

from recordings.cast_recorder import CastRecorder, assert_tables_column_aligned

_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
# GRIM_CASTS_DIR lets a test point collection at a fixture directory instead
# of the real casts/ tree.
_CASTS_DIR = Path(os.environ.get("GRIM_CASTS_DIR") or Path(__file__).resolve().parent / "casts")

# The site's own dark palette (the `:root` block in `docs/src/pages/
# index.astro`), identical for every cast -- a module constant rather than a
# per-cast YAML key. asciinema-player reads a cast's embedded `theme` header
# automatically when the `AsciinemaPlayer.create()` call passes no `theme`
# option (that page's `is:inline` script) -- this is the only way to get the
# player's background off its stock terminal themes and onto the page's own
# dark background. grim's plain output carries no ANSI color, so the 16-entry
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


# The one artifact a `seed:` entry publishes. One shape for every cast, so a
# reader who watches two of them sees the same skill rather than wondering
# what changed. `{name}` is the seeded repo's last path segment, which is
# also the directory the materializer renders into.
_SEED_SKILL = (
    "---\n"
    "name: {name}\n"
    "description: Greet the reader so a running agent has something to load.\n"
    "---\n"
    "# Hello world\n"
)


class CastScriptError(Exception):
    """A cast YAML is unusable: bad syntax, a missing required key, or a step
    that asserts nothing. Raised at import time, so pytest reports it as a
    collection error and no recording is attempted."""


@dataclass(frozen=True, slots=True)
class CastStep:
    type: str
    expect: str


@dataclass(frozen=True, slots=True)
class CastScript:
    path: Path          # the yaml file this came from
    output: Path        # absolute, resolved against _REPO_ROOT
    steps: tuple[CastStep, ...]
    title: str = ""
    registry: str = "local"
    columns: int = 180
    rows: int = 24
    dirs: tuple[str, ...] = ()   # workspace-relative dirs created before recording
    keys: tuple[tuple[str, float], ...] = ()   # (send, wait) pairs for the raw-stream last step
    raw: bool = False            # the last step is launched via CastRecorder.run_raw
    seed: tuple[str, ...] = ()   # OCI repos published into the session registry before the shell opens


_KEY_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*\Z")
_INT_RE = re.compile(r"-?[0-9]+\Z")
# A value *opening* with one of these is YAML the subset does not read: a
# block scalar (`|`, `>`), a flow collection (`[`, `{`), an anchor (`&`) or
# an alias (`*`). Only the FIRST character is ever checked, and that is
# deliberate -- both real casts look like parser bugs and are not:
#   title: "grim: one declaration, many clients"   -- a `: ` inside the
#     value, handled by splitting on the *first* colon and stripping quotes;
#   expect: skill\s+...@sha256:[0-9a-f]{64}\s+added -- colons and braces
#     *inside* the value, which is a regex, not a flow mapping.
_FLOW_INDICATORS = "|>[{&*"
_BLOCK_KEYS = ("steps", "dirs", "keys", "seed")
_ALLOWED_KEYS = {
    "output", "title", "registry", "columns", "rows", "dirs", "steps",
    "keys", "raw", "seed",
}

# `\e`, `\n` and `\t` -- the only two-character escapes a `keys:` item's
# `send` value may spell. A cast YAML has no escape processing anywhere
# else, so this table is the one place a control character can be written.
_SEND_ESCAPES = {"e": "\x1b", "n": "\n", "t": "\t"}


def _unquote(value: str) -> str:
    """Strip one matching pair of surrounding quotes. No escape processing."""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        return value[1:-1]
    return value


def _unescape_send(value: str) -> str:
    """Expand `\\e`, `\\n` and `\\t` in a `keys:` item's `send` value to the
    control characters `\\x1b`, `\\n` and `\\t`. Every other character,
    including a lone backslash, passes through unchanged -- a cast YAML has
    no escape processing anywhere else, so this is the one place a control
    character can be written.
    """
    out: list[str] = []
    i = 0
    while i < len(value):
        ch = value[i]
        if ch == "\\" and i + 1 < len(value) and value[i + 1] in _SEND_ESCAPES:
            out.append(_SEND_ESCAPES[value[i + 1]])
            i += 2
        else:
            out.append(ch)
            i += 1
    return "".join(out)


def _scalar(value: str) -> str | int:
    """A bare digit-run becomes an `int`; anything else a (unquoted) `str`."""
    return int(value) if _INT_RE.match(value) else _unquote(value)


def parse_cast_yaml(text: str, source: Path) -> dict[str, object]:
    """Parse the supported YAML subset. Raises CastScriptError naming
    *source* and the 1-based line number for anything outside it (block
    scalars, nested mappings, flow collections, anchors, multiple
    documents)."""

    def fail(n: int, message: str) -> None:
        raise CastScriptError(f"{source}:{n}: {message}")

    def strip_comment(n: int, value: str) -> str:
        """Drop a trailing ` # comment`, exactly as YAML does for a plain
        scalar. A quoted value keeps everything up to its closing quote --
        that is how a value containing ` #` is written. Absorbing the
        comment instead would make this subset disagree silently with every
        other YAML reader, which is the one thing it must never do.
        """
        if value[:1] in ('"', "'"):
            end = value.find(value[0], 1)
            if end == -1:
                fail(n, f"unterminated {value[0]!r} quote in {value!r}")
            return value[: end + 1]
        cut = value.find(" #")
        return value[:cut].rstrip() if cut != -1 else value

    def pair(n: int, body: str) -> tuple[str, str]:
        """Split `key: value` on the FIRST colon; validate both halves."""
        key, sep, value = body.partition(":")
        if not sep:
            fail(n, f"expected `key: value`, got {body!r}")
        key, value = key.strip(), strip_comment(n, value.strip())
        if not _KEY_RE.match(key):
            fail(n, f"{key!r} is not a valid key -- use letters, digits, `_` or `-`")
        if value and value[0] in _FLOW_INDICATORS:
            fail(
                n,
                f"a value starting with {value[0]!r} is a block scalar, flow "
                f"collection, anchor or alias, none of which this subset reads "
                f'-- quote it instead: {key}: "{value}"',
            )
        return key, value

    data: dict[str, object] = {}
    steps: list[dict[str, str]] = []
    dirs: list[str] = []
    keys: list[dict[str, str]] = []
    seed: list[str] = []
    scalar_blocks = {"dirs": dirs, "seed": seed}
    mapping_blocks = {"steps": steps, "keys": keys}
    all_blocks = {**scalar_blocks, **mapping_blocks}
    block: str | None = None
    block_indent = 0
    item_indent: int | None = None

    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        lead = raw[: len(raw) - len(raw.lstrip())]
        if "\t" in lead:
            fail(n, "tab in the leading indentation -- indent with spaces")
        indent = len(lead)
        if line in ("---", "..."):
            fail(n, f"{line!r} -- a cast script is one document with no directives")
        if block is not None and indent <= block_indent:
            block, item_indent = None, None

        if block is None:
            if line.startswith("-"):
                fail(n, f"a `- ` item outside a {'/'.join(f'`{k}:`' for k in _BLOCK_KEYS)} block")
            if indent:
                fail(n, "unexpected indentation -- nested mappings are not read; write `key: <scalar>` at column 0")
            key, value = pair(n, line)
            if key in data:
                fail(n, f"duplicate top-level key `{key}` -- declare it once")
            if value:
                data[key] = _scalar(value)
                continue
            if key not in _BLOCK_KEYS:
                fail(n, f"`{key}:` with no value opens a block, and only {'/'.join(_BLOCK_KEYS)} may -- write `{key}: <scalar>`")
            data[key] = all_blocks[key]
            block, block_indent, item_indent = key, indent, None
            continue

        is_item = line.startswith("- ")
        if is_item and item_indent is not None and indent > item_indent:
            fail(n, f"a nested `- ` sequence inside a `{block}:` item -- an item is one flat `key: value` mapping")
        if is_item:
            item_indent, body = indent, strip_comment(n, line[2:].strip())
        elif item_indent is None or indent <= item_indent:
            fail(n, "expected `- key: value` opening an item, or a more-indented `key: value` continuing the last one")
        else:
            body = line

        if block in scalar_blocks:
            if not is_item or not body or body[0] in _FLOW_INDICATORS or ": " in body:
                fail(n, f"a `{block}:` entry is one plain scalar per `- ` line (`  - .claude`), got {line!r}")
            scalar_blocks[block].append(_unquote(body))
            continue
        target = mapping_blocks[block]
        key, value = pair(n, body)
        if not value:
            fail(n, f"a `{block}:` item line must be `key: value`, got {line!r}")
        if is_item:
            target.append({key: _unquote(value)})
            continue
        if key in target[-1]:
            fail(n, f"duplicate key `{key}` in this `{block}:` item -- declare it once")
        target[-1][key] = _unquote(value)

    return data


def load_cast_script(path: Path) -> CastScript:
    """Read and validate one cast YAML. Raises CastScriptError for:

    - an unknown top-level key
    - a missing `output`
    - a missing or empty `steps`
    - a step with no `type`
    - a step with no `expect` -- the usage error D-8 calls out; the message
      names the yaml path and the step's command
    - `registry` outside `{local, public}`
    - `{registry}` used by a `registry: public` cast
    - a `keys:` item carrying a key other than `send`/`wait`, an empty or
      missing `send`, or a `wait` that does not parse as a non-negative float
    - `keys:` declared without `raw: true`
    - `raw` set to anything but the literal `true` or `false`
    - `seed:` declared on a `registry: public` cast -- there is no session
      registry to push a seed artifact into
    """
    data = parse_cast_yaml(path.read_text(encoding="utf-8"), path)

    unknown = sorted(set(data) - _ALLOWED_KEYS)
    if unknown:
        raise CastScriptError(
            f"{path}: unknown top-level key(s) {', '.join(unknown)} -- "
            f"allowed: {', '.join(sorted(_ALLOWED_KEYS))}"
        )
    if not data.get("output"):
        raise CastScriptError(f"{path}: no `output` -- every cast declares where its .cast is written")
    raw_steps = data.get("steps") or []
    if not raw_steps:
        raise CastScriptError(f"{path}: no `steps` -- a cast with nothing to run records nothing")

    registry = data.get("registry", "local")
    if registry not in ("local", "public"):
        raise CastScriptError(f"{path}: registry {registry!r} -- must be `local` or `public`")
    for key in ("columns", "rows"):
        if key in data and not isinstance(data[key], int):
            raise CastScriptError(f"{path}: `{key}` must be a whole number, got {data[key]!r}")

    raw_flag_value = data.get("raw")
    if raw_flag_value is not None and raw_flag_value not in ("true", "false"):
        raise CastScriptError(f"{path}: `raw` {raw_flag_value!r} -- must be `true` or `false`")
    raw_flag = raw_flag_value == "true"

    raw_seed = data.get("seed") or ()
    if raw_seed and registry == "public":
        raise CastScriptError(
            f"{path}: `seed:` on a `registry: public` cast -- there is no session "
            f"registry to push a seed artifact into"
        )

    raw_keys = data.get("keys") or []
    if raw_keys and not raw_flag:
        raise CastScriptError(f"{path}: `keys:` declared without `raw: true` -- nothing to send them to")
    keys: list[tuple[str, float]] = []
    for item in raw_keys:
        extra = sorted(set(item) - {"send", "wait"})
        if extra:
            raise CastScriptError(f"{path}: `keys:` item key(s) {', '.join(extra)} -- an item carries only `send` and `wait`")
        send = item.get("send")
        if not send:
            raise CastScriptError(f"{path}: a `keys:` item declares no `send` -- name the sequence it writes")
        wait_str = item.get("wait", "0.5")
        try:
            wait = float(wait_str)
        except ValueError:
            raise CastScriptError(f"{path}: `keys:` item's `wait` {wait_str!r} does not parse as a number") from None
        if wait < 0:
            raise CastScriptError(f"{path}: `keys:` item's `wait` {wait!r} -- must be >= 0")
        keys.append((_unescape_send(send), wait))

    steps: list[CastStep] = []
    for item in raw_steps:
        extra = sorted(set(item) - {"type", "expect"})
        if extra:
            raise CastScriptError(f"{path}: step key(s) {', '.join(extra)} -- a step carries only `type` and `expect`")
        command = item.get("type")
        if not command:
            raise CastScriptError(f"{path}: a step declares no `type` -- name the command it runs")
        expect = item.get("expect")
        if not expect:
            raise CastScriptError(
                f"{path}: step `{command}` declares no `expect` -- a cast that "
                f"asserts nothing cannot ship; add an `expect:` regex matching "
                f"what the command prints"
            )
        try:
            pattern = re.compile(expect)
        except re.error as exc:
            raise CastScriptError(
                f"{path}: step `{command}`'s expect {expect!r} is not a regex: {exc}"
            ) from exc
        # An empty *string* is not the only way to assert nothing: `.*`, `^`
        # and `x?` all match a silent command's zero bytes, and each one
        # produced a real .cast in review. A pattern that matches "" is
        # vacuous by construction, whatever the command prints.
        if pattern.search(""):
            raise CastScriptError(
                f"{path}: step `{command}`'s expect {expect!r} matches the empty "
                f"string -- it asserts nothing; name something the command prints"
            )
        if registry == "public" and "{registry}" in command:
            raise CastScriptError(
                f"{path}: step `{command}` uses `{{registry}}`, which only a "
                f"`registry: local` cast may -- there is no session host to substitute"
            )
        steps.append(CastStep(type=command, expect=expect))

    output = Path(str(data["output"]))
    return CastScript(
        path=path,
        output=output if output.is_absolute() else _REPO_ROOT / output,
        steps=tuple(steps),
        title=str(data.get("title", "")),
        registry=str(registry),
        columns=int(data.get("columns", 180)),
        rows=int(data.get("rows", 24)),
        dirs=tuple(data.get("dirs") or ()),
        keys=tuple(keys),
        raw=raw_flag,
        seed=tuple(raw_seed),
    )


def discover_cast_scripts() -> list[CastScript]:
    """Every `*.yaml` under _CASTS_DIR, sorted by name. Called at import time
    so a broken script fails collection."""
    if not _CASTS_DIR.is_dir():
        # A missing directory would otherwise parametrize over nothing --
        # a green run recording no casts at all.
        raise CastScriptError(f"no cast directory at {_CASTS_DIR}")
    return [load_cast_script(p) for p in sorted(_CASTS_DIR.glob("*.yaml"))]


def record_cast(script: CastScript, grim_binary: Path, registry_host: str | None) -> Path:
    """Record *script* and write its `.cast`. Returns the written path.

    In order: create a short flat `tempfile.TemporaryDirectory(prefix=
    "grim-cast-", dir="/tmp")` (never pytest's `tmp_path`, and never
    TMPDIR -- grim never truncates a table column, so a deep workspace path
    inflates the printed `Path` column of the committed asset), make
    `<tmp>/myproject` plus every entry in `script.dirs`, build a
    `GrimRunner(grim_binary, <tmp>/grim-home, cwd=project_dir)` and prepend
    `grim_binary.parent` to its env `PATH` so the recording types the bare
    command name, substitute `{registry}` in each step's `type`, drive a
    `CastRecorder` over the steps, assert each step's `expect` regex matches
    `CastRecorder.stripped(output)` (`re.search`), then
    `build(title=..., theme=_THEME).auto_height().write(script.output)`, and
    finally -- for a `registry: local` script -- assert the written cast
    text contains no `ghcr.io`.

    A script carrying `script.seed` publishes each repo before the shell
    opens: `make_artifact(repo, "skill", {f"{repo.rsplit('/', 1)[-1]}/
    SKILL.md": <frontmatter>}, tag="latest")`, so a `registry: local` cast
    has something to `grim add` without a visible, noise-adding
    `grim release` step of its own.

    A script carrying `script.raw` drives its **last** step through
    `CastRecorder.run_raw` instead of `run_command`, passing `script.keys`
    as the `(seq, wait)` schedule, and matches that step's `expect` against
    `CastRecorder.stripped` of the returned raw stream rather than a
    line-buffered command's output. A raw script also skips `auto_height()`:
    a full-screen application paints with cursor moves rather than newlines,
    so counting newlines would shrink the cast to a few rows and clip the
    frame the reader came to see. Its declared `rows` stands.
    """
    for repo in script.seed:
        name = repo.rsplit("/", 1)[-1]
        make_artifact(
            repo, "skill", {f"{name}/SKILL.md": _SEED_SKILL.format(name=name)}, tag="latest"
        )

    # `dir="/tmp"` deliberately ignores TMPDIR: the workspace path is *part
    # of the asset* (grim prints it in `grim init`'s Path column and never
    # truncates a column to fit), so an inherited deep TMPDIR silently
    # widens the recorded table instead of failing. Hardcoding a short root
    # costs nothing here -- `CastRecorder` already spawns `/bin/bash`, so
    # this harness is POSIX-only either way.
    with tempfile.TemporaryDirectory(prefix="grim-cast-", dir="/tmp") as tmp:
        base = Path(tmp)
        project_dir = base / "myproject"
        project_dir.mkdir()
        for relative in script.dirs:
            (project_dir / relative).mkdir(parents=True, exist_ok=True)

        grim_home = base / "grim-home"
        grim_home.mkdir()
        runner = GrimRunner(grim_binary, grim_home, cwd=project_dir)
        env = dict(runner.env)
        # So the recording types the bare command name ("grim init") instead
        # of the absolute build path.
        env["PATH"] = f"{grim_binary.parent}:{env['PATH']}"

        recorder = CastRecorder(
            env=env,
            cwd=str(project_dir),
            width=script.columns,
            height=script.rows,
            raw_stream=script.raw,
        )
        recorder.open()
        try:
            last = len(script.steps) - 1
            for index, step in enumerate(script.steps):
                # `str.replace`, never `str.format`: a step's command
                # legitimately carries braces of its own (`find ... -exec {} \;`,
                # a `{64}` regex quantifier), and `format` would raise on them.
                command = (
                    step.type
                    if registry_host is None
                    else step.type.replace("{registry}", registry_host)
                )
                # Only the last step of a `raw:` script is the full-screen one;
                # everything before it is an ordinary line-buffered command.
                if script.raw and index == last:
                    captured = recorder.run_raw(command, list(script.keys))
                else:
                    captured = recorder.run_command(command)
                output = CastRecorder.stripped(captured)
                # Asserted per step, *before* anything is written: a failed
                # cast must leave no half-true committed asset behind.
                if not re.search(step.expect, output):
                    raise AssertionError(
                        f"{script.path}: step `{command}` did not match its `expect`\n"
                        f"  expect: {step.expect}\n"
                        f"  output: {output!r}"
                    )
        finally:
            # Even on a failed step -- otherwise the bash child outlives the run.
            recorder.close()

        recording = recorder.build(title=script.title, theme=_THEME)
        if not script.raw:
            recording = recording.auto_height()
        # Checked on the built text, not on the written file: a local cast that
        # reached GHCR must leave no polluted asset on disk either.
        if script.registry == "local" and "ghcr.io" in recording.to_cast():
            raise AssertionError(
                f"{script.path} is `registry: local` but its cast names ghcr.io -- "
                f"C-029: only own-index.cast reaches the public registry"
            )
        recording.write(script.output)

    return script.output


@pytest.mark.parametrize("script", discover_cast_scripts(), ids=lambda s: s.path.name)
def test_record_cast(script: CastScript, grim_binary: Path, registry: str) -> None:
    """Assert the `.cast` is written, its first line parses as JSON with
    `"version": 2`, and `assert_tables_column_aligned` runs on it.

    This is the test that produces the committed assets -- `docs/public/
    demo.cast` today, the eleven casts under `docs/public/casts/` later.
    """
    written = record_cast(
        script, grim_binary, registry if script.registry == "local" else None
    )

    assert written == script.output
    assert written.is_file(), f"{written} was not written"

    header = json.loads(written.read_text(encoding="utf-8").splitlines()[0])
    assert header["version"] == 2

    # The column-alignment guard runs on every cast this harness produces --
    # it is what stops a post-hoc string substitution (header padded for one
    # string, row carrying a shorter one) from shipping. See
    # `test_assert_tables_column_aligned_catches_a_misaligned_row` for the
    # proof that this guard can still fail.
    assert_tables_column_aligned(written)


# ---------------------------------------------------------------------------
# Fixture builders and the nested-pytest helper the collection tests share.
# Everything below only writes YAML into `tmp_path` or runs a subprocess --
# no cast fixture is ever committed.
# ---------------------------------------------------------------------------

# test/recordings/test_record_casts.py -> recordings -> test
_TEST_DIR = Path(__file__).resolve().parent.parent

# The skill tree `grim add` accepts, copied from `tests/test_add_remove.py`.
_SKILL_FILES = {
    "code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"
}


def _write_yaml(path: Path, lines: list[str]) -> Path:
    """Write a cast YAML built from *lines* and return its path."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def _nested_pytest(
    *args: str, casts_dir: Path | None = None
) -> subprocess.CompletedProcess[str]:
    """Run pytest as a child process from `test/` and capture its output.

    The child inherits this session's environment **minus**
    ``_GRIM_FRESH_REGISTRY_NAME``. That variable names the throwaway
    ``registry:2`` container the *parent* session started, and the child's
    ``pytest_unconfigure`` (`test/conftest.py`) would ``docker rm -f`` it on
    exit -- destroying the registry every surrounding test in this run
    depends on. Every nested pytest invocation in this module must go
    through this helper for that reason alone.

    ``GRIM_TEST_REGISTRY_HOST`` and ``_GRIM_REGISTRY_VERIFIED`` are kept, so
    the child reuses the parent's registry rather than probing for or
    starting another one.
    """
    env = os.environ.copy()
    env.pop("_GRIM_FRESH_REGISTRY_NAME", None)
    # pytest sizes its report lines to COLUMNS; the assertions below look for
    # whole file paths and whole commands, so give it room not to wrap.
    env["COLUMNS"] = "1000"
    if casts_dir is not None:
        env["GRIM_CASTS_DIR"] = str(casts_dir)
    return subprocess.run(
        [sys.executable, "-m", "pytest", *args],
        cwd=str(_TEST_DIR),
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=600,
    )


def test_expect_that_never_matches_fails_the_recording(
    tmp_path: Path, grim_binary: Path
) -> None:
    """**This is the test that proves the harness is not vacuous.**

    Every other cast test asserts that a recording succeeded. If a step's
    `expect` were never actually matched against the captured output, all of
    them would pass over nothing and the eleven casts built on this contract
    would document whatever grim happened to print. So: one step whose
    `expect` provably cannot match, and `record_cast` must refuse -- with a
    message naming the pattern and the command, and with no `.cast` written.

    The step deliberately runs `echo`, not `grim`: the failure being proven
    is the harness's own assertion, so it needs no registry, no network and
    no grim binary behaviour.
    """
    out = tmp_path / "red.cast"
    script = load_cast_script(
        _write_yaml(
            tmp_path / "red.yaml",
            [
                f"output: {out}",
                "registry: public",
                "steps:",
                "  - type: echo cast-harness-probe",
                "    expect: this-pattern-never-appears-in-any-output",
            ],
        )
    )

    with pytest.raises(AssertionError) as excinfo:
        record_cast(script, grim_binary, None)

    message = str(excinfo.value)
    assert "this-pattern-never-appears-in-any-output" in message, message
    assert "echo cast-harness-probe" in message, message
    assert not out.exists(), "a failed recording must not leave a .cast behind"


@pytest.mark.parametrize("expect", [".*", "^", "x?", "(a)?"])
def test_expect_matching_the_empty_string_is_rejected(tmp_path: Path, expect: str) -> None:
    """An empty *match* asserts as little as an empty *string*.

    Rejecting only a missing `expect` left `.*` as a legal way to record a
    cast over zero output -- verified in review: each of these produced a
    real .cast against a silent command. The check is `pattern.search("")`,
    so it holds for any pattern however it is spelled.
    """
    script = _write_yaml(
        tmp_path / "vacuous.yaml",
        [f"output: {tmp_path / 'out.cast'}", "steps:", "  - type: true", f'    expect: "{expect}"'],
    )
    with pytest.raises(CastScriptError, match="matches the empty string"):
        load_cast_script(script)


def test_unreadable_yaml_is_rejected(tmp_path: Path) -> None:
    """The subset disagreeing silently with a real YAML reader is the one
    failure mode it must not have, so each disagreement is an error naming
    its line: an unparsable regex, a trailing comment absorbed into a value,
    a nested sequence flattened into a sibling step, a duplicate step key
    last-winning.
    """
    def load(name: str, *lines: str) -> str:
        with pytest.raises(CastScriptError) as excinfo:
            load_cast_script(_write_yaml(tmp_path / name, list(lines)))
        return str(excinfo.value)

    out = f"output: {tmp_path / 'out.cast'}"
    assert "is not a regex" in load("badre.yaml", out, "steps:", "  - type: true", "    expect: a(b")
    assert "nested `- ` sequence" in load(
        "nested.yaml", out, "steps:", "  - type: true", "    expect: x", "    - type: false"
    )
    assert "duplicate key `expect`" in load(
        "dupe.yaml", out, "steps:", "  - type: true", "    expect: x", "    expect: y"
    )
    # ` # comment` is a comment to every YAML reader. Absorbing it would
    # write the cast to a path ending in `  # the landing hero` and leave
    # the real asset stale, green.
    assert load_cast_script(
        _write_yaml(
            tmp_path / "ok.yaml",
            [f"output: {tmp_path / 'out.cast'}  # the landing hero", "steps:", "  - type: true", "    expect: x"],
        )
    ).output == tmp_path / "out.cast"


def test_recording_workspace_ignores_a_deep_tmpdir(
    tmp_path: Path, grim_binary: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A deep `TMPDIR` must not reach the recorded terminal.

    The workspace path is part of the asset: `grim init` prints it in a
    `Path` column and grim never truncates a column to fit. An inherited
    deep TMPDIR -- every agent worktree in this repo exports one -- widens
    that column by ~100 characters, and the recording stays internally
    *aligned*, so `assert_tables_column_aligned` cannot catch it. Nothing
    would fail; a corrupted asset would simply be committed.

    The step is `pwd`, so this needs no registry, no network and no grim.
    """
    # `tempfile.tempdir`, not `TMPDIR`: `gettempdir()` caches its answer on
    # first use, so by the time any test runs, setting the environment
    # variable changes nothing. `tempdir` is the documented override, and it
    # is exactly what an exported TMPDIR would have become.
    #
    # The leaf is `scratch`, not `tmp`: a directory named `tmp` would put the
    # substring `/tmp/grim-cast-` back into the deep path and the regex below
    # would match even with the bug present.
    deep = tmp_path / "a-very-deeply-nested" / "agent" / "scratchpad" / "scratch"
    deep.mkdir(parents=True)
    monkeypatch.setattr(tempfile, "tempdir", str(deep))

    out = tmp_path / "pwd.cast"
    script = load_cast_script(
        _write_yaml(
            tmp_path / "pwd.yaml",
            [
                f"output: {out}",
                "registry: public",
                "steps:",
                "  - type: pwd",
                r"    expect: /tmp/grim-cast-\w+/myproject",
            ],
        )
    )

    record_cast(script, grim_binary, None)

    assert str(deep) not in out.read_text(encoding="utf-8"), (
        "TMPDIR leaked into the recorded terminal"
    )


def test_step_without_expect_fails_collection(tmp_path: Path) -> None:
    """A step that asserts nothing is a usage error (D-8), and it must be
    raised at *collection* time -- not by anyone remembering to call the
    validator. Proven by collecting this very module in a child pytest with
    `GRIM_CASTS_DIR` pointed at a fixture directory holding the bad YAML.
    """
    casts_dir = tmp_path / "casts"
    _write_yaml(
        casts_dir / "noexpect.yaml",
        [
            "output: docs/public/never-written.cast",
            "registry: public",
            "steps:",
            "  - type: echo missing-expect-probe",
        ],
    )

    result = _nested_pytest(
        "--collect-only",
        "-q",
        "-p",
        "no:cacheprovider",
        "recordings/test_record_casts.py",
        casts_dir=casts_dir,
    )
    combined = result.stdout + result.stderr

    assert result.returncode != 0, combined
    assert "noexpect.yaml" in combined, combined
    assert "expect" in combined, combined
    assert "echo missing-expect-probe" in combined, combined


def test_local_registry_cast_records_against_the_session_registry(
    tmp_path: Path, grim_binary: Path, registry: str
) -> None:
    """A `registry: local` cast drives grim against the session's own
    `registry:2` fixture, substitutes `{registry}` for its host, and leaks no
    `ghcr.io` reference into the committed asset.
    """
    # Same UUID-prefixed shape as the `unique_repo` fixture; generated here
    # because this module cannot add a fixture to `test/conftest.py`.
    repo = f"grim-test/{uuid.uuid4().hex[:12]}/code-review"
    # `latest`: `grim add <ref>` with no tag resolves the default tag.
    make_artifact(repo, "skill", _SKILL_FILES, tag="latest")

    out = tmp_path / "local.cast"
    script = load_cast_script(
        _write_yaml(
            tmp_path / "local.yaml",
            [
                f"output: {out}",
                "registry: local",
                # A client marker, so `grim add` has something to fan out to.
                "dirs:",
                "  - .claude",
                "steps:",
                "  - type: grim init",
                # `<tmp>/myproject/grimoire.toml  project  created`
                r"    expect: grimoire\.toml\s+project\s+created",
                "  - type: grim add {registry}/" + repo,
                # `skill  code-review  <host>/<repo>@sha256:<64hex>  added`
                r"    expect: skill\s+code-review\s+\S+@sha256:[0-9a-f]{64}\s+added",
            ],
        )
    )

    written = record_cast(script, grim_binary, registry)

    assert written == out
    text = written.read_text(encoding="utf-8")
    assert "ghcr.io" not in text, "a local-registry cast must not reach the public registry"
    assert registry in text, f"{registry} never appears in the recording"


def test_assert_tables_column_aligned_catches_a_misaligned_row(tmp_path: Path) -> None:
    """The alignment guard must still be able to fail.

    `assert_tables_column_aligned` runs on every cast this harness writes; a
    guard that silently became a no-op would pass every one of them. Both
    directions are asserted here, so neither an always-pass nor an
    always-fail implementation survives.

    `Kind  Name  Status` puts its columns at offsets 0, 6 and 12.
    """
    header = "Kind  Name  Status\r\n"

    def build(name: str, row: str) -> Path:
        path = tmp_path / name
        path.write_text(
            "\n".join(
                [
                    json.dumps({"version": 2, "width": 80, "height": 6, "title": ""}),
                    json.dumps([0.0, "o", header]),
                    json.dumps([0.1, "o", row]),
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        return path

    # Columns at 0, 6, 12 -- matches the header exactly.
    assert_tables_column_aligned(build("aligned.cast", "skill x     ok\r\n"))

    # Columns at 0, 7, 10 -- the shape a post-hoc string substitution leaves.
    with pytest.raises(AssertionError):
        assert_tables_column_aligned(build("misaligned.cast", "skill  x  ok\r\n"))


def test_recordings_stay_outside_testpaths() -> None:
    """`task verify` / a bare `pytest` must never collect a recording.

    `testpaths = ["tests"]` (`test/pyproject.toml`) is the only thing keeping
    a quality gate from spawning PTYs and rewriting committed `.cast` assets,
    so it is asserted rather than assumed. The run's own success is asserted
    too: "nothing under recordings/" must not be reachable by a broken
    invocation that collected nothing at all.
    """
    result = _nested_pytest("--collect-only", "-q", "-p", "no:cacheprovider")
    combined = result.stdout + result.stderr

    assert result.returncode in (0, 5), combined
    node_ids = [line.strip() for line in result.stdout.splitlines() if "::" in line]
    assert node_ids, f"collected nothing at all:\n{combined}"
    assert not [n for n in node_ids if n.startswith("recordings/")], (
        "a bare pytest collected a recording:\n"
        + "\n".join(n for n in node_ids if n.startswith("recordings/"))
    )
    assert [n for n in node_ids if n.startswith("tests/")], (
        f"collected no acceptance tests at all:\n{combined}"
    )


def test_a_prompt_sentinel_split_across_chunks_never_reaches_the_cast() -> None:
    """The raw-stream drain reads fixed-size chunks, so the shell's prompt
    sentinel routinely straddles two of them. Emitting the first half would
    paint `___CAST_PROM` into a committed screencast, and a frame already
    emitted cannot be taken back -- which is why the partial is held instead
    of merely `replace`d per chunk.
    """
    recorder = CastRecorder.__new__(CastRecorder)
    sentinel = CastRecorder._SENTINEL

    assert recorder._without_sentinel(f"a{sentinel}b") == ("ab", "")

    emit, held = recorder._without_sentinel("a" + sentinel[:10])
    assert (emit, held) == ("a", sentinel[:10])
    assert recorder._without_sentinel(held + sentinel[10:] + "b") == ("b", "")

    # No next chunk on the final call, so a partial is content after all.
    assert recorder._without_sentinel("a" + sentinel[:10], final=True) == (
        "a" + sentinel[:10],
        "",
    )
    # Ordinary text is never held back.
    assert recorder._without_sentinel("plain text") == ("plain text", "")
