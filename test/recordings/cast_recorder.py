# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Minimal asciicast (v2) recorder for driving the real ``grim`` binary
through a PTY and writing a ``.cast`` file an asciinema-compatible player
can replay.

Ported and trimmed from the OCX project's ``test/recordings/cast_recorder.py``
(a much larger tested-doc-command pipeline — see
``.agents/research/research_promotion_positioning.md`` "Demo asset" and the
W6 plan for the source). grim's plain-text tables carry no ANSI color and no
progress-spinner redraws (no ``comfy-table``/``indicatif`` in this codebase —
confirmed by manual PTY probe before writing this), so the color-aware
table-realignment and spinner-stripping machinery in the OCX original is not
ported: there is nothing for it to do here.

The OCX original also carried post-hoc string sanitization (swap a throwaway
registry host / tmp path for a clean public one after the fact). That is
gone too, and deliberately not ported: ``print_table`` (``src/cli/printer.rs``)
pads every column to the width of what it *actually rendered*, so replacing
a captured cell with a shorter string after the fact leaves the header
padding sized for the original, longer text — every column downstream falls
short of its own header. The fix is at the source, not in this file: the
caller (``test_record_demo.py``) drives grim against a real, anonymously
pullable registry with an unshortened digest and a short cwd, so nothing
ever needs rewriting. ``assert_tables_column_aligned`` below is the
regression guard for that invariant.
"""
from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

import pexpect


@dataclass
class CastEvent:
    timestamp: float
    event_type: str
    data: str


@dataclass
class CastRecording:
    width: int = 100
    height: int = 24
    title: str = ""
    events: list[CastEvent] = field(default_factory=list)
    # asciicast v2 optional header field (fg/bg/16-color palette). Lets
    # `agg --theme custom` render the GIF in the landing page's own dark
    # palette instead of a stock terminal theme.
    theme: dict[str, str] | None = None

    def to_cast(self) -> str:
        header_dict = {
            "version": 2,
            "width": self.width,
            "height": self.height,
            "title": self.title,
        }
        if self.theme:
            header_dict["theme"] = self.theme
        header = json.dumps(header_dict)
        lines = [header]
        for event in self.events:
            lines.append(json.dumps([round(event.timestamp, 3), event.event_type, event.data]))
        return "\n".join(lines) + "\n"

    def auto_height(self, padding: int = 2, minimum: int = 5) -> CastRecording:
        """Set height based on the actual number of lines in the recording."""
        max_y = 0
        y = 0
        for event in self.events:
            if event.event_type != "o":
                continue
            for char in event.data:
                if char == "\n":
                    y += 1
                    max_y = max(max_y, y)
        self.height = max(max_y + padding, minimum)
        return self

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.to_cast())


class CastRecorder:
    """Records CLI interactions as an asciicast v2 file.

    Spawns a persistent bash shell through a PTY so ``grim`` inherits a real
    terminal environment. Typing uses simulated timing (deterministic);
    command output uses real-time capture.
    """

    _SENTINEL = "___CAST_PROMPT_a7b3c9___"

    def __init__(
        self,
        env: dict[str, str],
        cwd: str,
        *,
        # grim never truncates its own table columns, so the PTY must be
        # wide enough to hold the full, unshortened digest -- there is no
        # post-hoc rewriting left to un-wrap a line the terminal already
        # hard-wrapped. `grim status`'s widest row
        # (Kind/Name/Source/Pinned@sha256-64hex/State) measures ~147 cols
        # for the real `ghcr.io/grimoire-rs/skills/grim-usage` ref; 180
        # leaves headroom.
        width: int = 180,
        height: int = 24,
        prompt: str = "$ ",
        typing_delay: float = 0.04,
        inter_command_delay: float = 1.0,
        output_delay: float = 0.3,
        end_pause: float = 2.0,
        raw_stream: bool = False,
    ):
        # In raw-stream mode the recorder emits frames straight from
        # `read_nonblocking` with wall-clock timestamps and leaves
        # alternate-screen escape sequences in the cast, because asciicast v2
        # stores the stream and the player's own terminal emulator paints it
        # (spec contract C-030, plan decision D-3).
        self.env = env
        self.cwd = cwd
        self.width = width
        self.height = height
        self.prompt = prompt
        self.typing_delay = typing_delay
        self.inter_command_delay = inter_command_delay
        self.output_delay = output_delay
        self.end_pause = end_pause
        self.raw_stream = raw_stream
        self._events: list[CastEvent] = []
        self._clock: float = 0.0
        self._shell: pexpect.spawn | None = None

    def open(self) -> None:
        """Start a persistent interactive bash shell for recording."""
        self._shell = pexpect.spawn(
            "/bin/bash",
            ["--norc", "--noprofile"],
            env=self.env,
            cwd=self.cwd,
            dimensions=(self.height, self.width),
            timeout=60,
            encoding="utf-8",
        )
        self._shell.sendline("stty -echo")
        self._shell.sendline(f'PS1="{self._SENTINEL}"')
        self._shell.expect_exact(self._SENTINEL)

    def close(self) -> None:
        if self._shell is not None:
            self._shell.sendline("exit")
            self._shell.close()
            self._shell = None

    def _emit(self, data: str) -> None:
        self._events.append(CastEvent(timestamp=self._clock, event_type="o", data=data))

    def type_command(self, command: str) -> None:
        """Simulate typing a command character by character."""
        self._emit(self.prompt)
        for char in command:
            self._clock += self.typing_delay
            self._emit(char)
        self._clock += self.typing_delay
        self._emit("\r\n")

    def _read_until_prompt(self, timeout: int = 60, *, emit: bool = True) -> str:
        """Read real-time output from the shell until the prompt sentinel appears."""
        assert self._shell is not None
        sentinel = self._SENTINEL
        sentinel_len = len(sentinel)
        buffer = ""
        emitted_up_to = 0
        wall_start = time.monotonic()
        clock_base = self._clock + self.output_delay

        while True:
            try:
                chunk = self._shell.read_nonblocking(size=4096, timeout=0.1)
                if chunk:
                    elapsed = time.monotonic() - wall_start
                    self._clock = clock_base + elapsed
                    buffer += chunk

                    idx = buffer.find(sentinel)
                    if idx >= 0:
                        remaining = buffer[emitted_up_to:idx]
                        if remaining and emit:
                            self._emit(remaining)
                        return buffer[:idx]

                    safe_end = len(buffer) - sentinel_len
                    if safe_end > emitted_up_to and emit:
                        to_emit = buffer[emitted_up_to:safe_end]
                        self._emit(to_emit)
                        emitted_up_to = safe_end
            except pexpect.TIMEOUT:
                if time.monotonic() - wall_start > timeout:
                    raise TimeoutError(f"Command timed out after {timeout}s. Buffer so far: {buffer!r}") from None
            except pexpect.EOF:
                remaining = buffer[emitted_up_to:]
                if remaining and emit:
                    self._emit(remaining)
                return buffer

    def run_command(self, cmd: str, *, timeout: int = 60) -> str:
        """Type and execute *cmd* in the persistent shell.

        Returns the captured output. Raises AssertionError on non-zero exit.
        """
        assert self._shell is not None, "call open() before run_command()"

        self.type_command(cmd)
        self._shell.sendline(cmd)
        output = self._read_until_prompt(timeout)

        saved_clock = self._clock
        self._shell.sendline("echo $?")
        rc_output = self._read_until_prompt(5, emit=False)
        self._clock = saved_clock

        rc_str = re.sub(r"\x1b\[[^a-zA-Z]*[a-zA-Z]", "", rc_output).strip()
        if rc_str and rc_str != "0":
            raise AssertionError(f"Command failed (rc={rc_str}): {cmd}\nOutput: {output}")

        self._clock += self.inter_command_delay
        return output

    def _answer_cursor_queries(self, chunk: str) -> None:
        """Reply to every cursor-position report (``ESC [ 6 n``) in *chunk*.

        crossterm asks the terminal where the cursor is before it enters the
        alternate screen, and gives up when nothing answers. Under a PTY the
        *driver* is the terminal, so it has to answer itself: without this,
        `grim tui` dies with "The cursor position could not be read within a
        normal duration" and the recording is 200 bytes of nothing (measured
        before this line existed). The row/column reported is irrelevant --
        ratatui redraws the whole frame from its own buffer.
        """
        assert self._shell is not None
        for _ in _CURSOR_QUERY_RE.findall(chunk):
            self._shell.send("\x1b[1;1R")

    def _without_sentinel(self, pending: str, *, final: bool = False) -> tuple[str, str]:
        """Split *pending* into (text safe to emit now, text held back).

        Complete prompt sentinels are dropped: the prompt is a recording
        artifact, and `type_command` paints the visible `$ ` itself. A
        *partial* sentinel at the end is held back, because the rest of it
        arrives in the next chunk and a half-prompt already emitted cannot
        be taken back. On the final call there is no next chunk, so nothing
        is held.
        """
        text = pending.replace(self._SENTINEL, "")
        if final:
            return text, ""
        for size in range(min(len(self._SENTINEL) - 1, len(text)), 0, -1):
            if text.endswith(self._SENTINEL[:size]):
                return text[:-size], text[-size:]
        return text, ""

    def _drain(self, seconds: float) -> str:
        """Read for *seconds*, emitting what arrives as frames at their real
        elapsed offset, and return the raw text.

        The frame-per-chunk shape is what `raw_stream` means: a full-screen
        application paints with cursor moves rather than lines, so there is
        no prompt sentinel to read up to and no line boundary to split on.
        The sentinel is stripped from what is *emitted* but kept in what is
        returned, so the caller can still tell the shell came back.
        """
        assert self._shell is not None
        captured: list[str] = []
        pending = ""
        wall_start = time.monotonic()
        clock_base = self._clock

        while True:
            remaining = seconds - (time.monotonic() - wall_start)
            if remaining <= 0:
                break
            try:
                chunk = self._shell.read_nonblocking(size=4096, timeout=min(0.05, remaining))
            except pexpect.TIMEOUT:
                continue
            except pexpect.EOF:
                break
            if not chunk:
                continue
            self._clock = clock_base + (time.monotonic() - wall_start)
            captured.append(chunk)
            self._answer_cursor_queries(chunk)
            emit, pending = self._without_sentinel(pending + chunk)
            if emit:
                self._emit(emit)

        emit, _ = self._without_sentinel(pending, final=True)
        if emit:
            self._emit(emit)
        # The viewer sees the whole pause, not just the part output arrived in.
        self._clock = clock_base + seconds
        return "".join(captured)

    def send_keys(self, seq: str, wait: float = 0.5) -> str:
        """Write the raw sequence *seq* to the PTY without a trailing newline,
        then drain output for *wait* seconds, emitting each chunk as a frame
        at its real elapsed offset. Returns what was captured.

        For driving a full-screen application (`grim tui`) that never
        returns to the shell prompt between key presses.
        """
        assert self._shell is not None, "call open() before send_keys()"
        self._shell.send(seq)
        return self._drain(wait)

    def run_raw(self, cmd: str, keys: "list[tuple[str, float]]", *, timeout: int = 60) -> str:
        """Type and launch *cmd*, then send each `(seq, wait)` pair in order
        via `send_keys`, and return the whole captured raw stream once the
        shell prompt sentinel comes back (the application exited).

        Unlike `run_command` this does not check `$?`, because the last key
        is the one that quits the application. The caller asserts against
        `CastRecorder.stripped(...)` of the return value, never against an
        emulated screen -- `pyte`, the usual Python emulator, has had no
        release since 2023-11 and mishandles the alternate screen buffer
        ratatui enters.
        """
        assert self._shell is not None, "call open() before run_raw()"
        if not self.raw_stream:
            raise AssertionError(
                "run_raw() needs a recorder opened with raw_stream=True -- "
                "without it nothing answers crossterm's cursor-position query "
                "and the application never starts"
            )

        self.type_command(cmd)
        self._shell.sendline(cmd)
        captured = [self._drain(self.output_delay)]
        for seq, wait in keys:
            captured.append(self.send_keys(seq, wait))
        # The last key is the one that quits, so the prompt is usually already
        # back inside that key's own wait. Reading again would then block until
        # the timeout on a prompt that will never come a second time.
        if self._SENTINEL not in "".join(captured):
            captured.append(self._read_until_prompt(timeout))
        self._clock += self.inter_command_delay
        return "".join(captured)

    def pause(self, seconds: float) -> None:
        self._clock += seconds

    def build(self, title: str = "", theme: dict[str, str] | None = None) -> CastRecording:
        # Add a final empty event so the player holds the last frame visible.
        events = list(self._events)
        self._clock += self.end_pause
        events.append(CastEvent(self._clock, "o", ""))
        return CastRecording(width=self.width, height=self.height, title=title, theme=theme, events=events)

    @staticmethod
    def stripped(text: str) -> str:
        """Return *text* with terminal escape sequences removed.

        A cast step's `expect` regex asserts against the escape-stripped
        stream, not the raw captured output -- a step's `run_command` output
        can carry cursor moves, mode-set/reset sequences and charset-select
        codes even though grim's own tables carry no color (see the module
        docstring). Strips CSI sequences (`\\x1b[...<final-byte>]`), G0/G1
        charset-select sequences (`\\x1b(0`, `\\x1b)B`, ...), and the
        DECPAM/DECPNM keypad-mode pair (`\\x1b=`, `\\x1b>`) via
        `r"\\x1b\\[[0-9;?]*[A-Za-z]|\\x1b[()][A-Z0-9]|\\x1b[=>]"` (spec
        contract C-030).
        """
        return _ESCAPE_RE.sub("", text)


# A grim table header: 2+ capitalized words separated by print_table's GAP
# (two spaces, `src/cli/printer.rs`) or more (extra column padding). Never
# matches a data row -- every cell grim renders in this demo (kind, name,
# digest ref, path, status word) is a single token, and a data row mixing a
# lowercase digest/path with a capitalized status word never satisfies "every
# token capitalized".
_TABLE_HEADER_RE = re.compile(r"^[A-Z][A-Za-z]*(?:  +[A-Z][A-Za-z]*)+$")
_TOKEN_RE = re.compile(r"\S+")
_ANSI_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")
# Terminal escapes `CastRecorder.stripped` removes before a step's `expect`
# regex is matched (spec contract C-030): CSI sequences, G0/G1 charset
# selects, and the DECPAM/DECPNM keypad-mode pair.
_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b[()][A-Z0-9]|\x1b[=>]")
# crossterm's cursor-position report request, in both the plain and the
# private-parameter spelling. `CastRecorder._answer_cursor_queries` replies.
_CURSOR_QUERY_RE = re.compile(r"\x1b\[\??6n")


def assert_tables_column_aligned(cast_path: Path) -> None:
    """Assert every plain-text table in the ``.cast`` at *cast_path* has its
    data columns starting at the same character offset as their header.

    Parses the asciicast v2 JSON lines, concatenates every output ("o")
    event into one terminal stream, and strips ANSI escapes (grim's plain
    output carries none, but a recording is not guaranteed to stay that
    way). Any line matching the header shape opens a table; every following
    non-blank, non-prompt line must start each of its columns at the same
    offset as the header's.

    Regression guard: this must fail against a recording built with
    post-hoc string substitution (a header sized for a long, real
    registry-host/digest/path, followed by a row shortened to a friendlier
    display string) and pass against one where grim renders the displayed
    text directly -- see the module docstring and `test_record_demo.py`.
    """
    text = "".join(
        _ANSI_RE.sub("", event[2])
        for raw_line in cast_path.read_text().splitlines()
        if isinstance(event := json.loads(raw_line), list) and event[1] == "o"
    )
    lines = text.replace("\r\n", "\n").replace("\r", "").split("\n")

    misaligned: list[str] = []
    i = 0
    while i < len(lines):
        header = lines[i]
        if not _TABLE_HEADER_RE.match(header):
            i += 1
            continue
        header_starts = [m.start() for m in _TOKEN_RE.finditer(header)]
        j = i + 1
        while j < len(lines) and lines[j].strip() and not lines[j].startswith("$ ") and not _TABLE_HEADER_RE.match(lines[j]):
            row_starts = {m.start() for m in _TOKEN_RE.finditer(lines[j])}
            for col, h_start in enumerate(header_starts):
                if h_start not in row_starts:
                    misaligned.append(f"header {header!r} col {col} starts at {h_start}, row {lines[j]!r} has no cell there")
            j += 1
        i = j

    assert not misaligned, "misaligned table column(s) in " + str(cast_path) + ":\n" + "\n".join(misaligned)
