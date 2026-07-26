# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Minimal asciicast (v2) recorder for driving the real ``grim`` binary
through a PTY and writing a ``.cast`` file an asciinema-compatible player
can replay.

Ported and trimmed from the OCX project's ``test/recordings/cast_recorder.py``
(a much larger tested-doc-command pipeline — see
``.claude/artifacts/research_promotion_positioning.md`` "Demo asset" and the
W6 plan for the source). grim's plain-text tables carry no ANSI color and no
progress-spinner redraws (no ``comfy-table``/``indicatif`` in this codebase —
confirmed by manual PTY probe before writing this), so the color-aware
table-realignment and spinner-stripping machinery in the OCX original is not
ported: there is nothing for it to do here. What's kept is the generic core
— simulated-typing capture, real-time output capture, and post-hoc string
sanitization (hide the throwaway local registry host and tmp paths behind
the clean public reference a reader would actually type).
"""
from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

import pexpect

# One SGR-free digest ref, as grim prints it: `@sha256:<64 hex>`. Shortened
# for a readable recording; grim itself never truncates.
_DIGEST_REF_RE = re.compile(r"@sha256:([a-f0-9]{12})[a-f0-9]{52}")


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

    def sanitize(self, replacements: dict[str, str]) -> CastRecording:
        """Replace literal strings in all event data (registry host, tmp paths)."""
        self._merge_close_events()
        for event in self.events:
            for old, new in replacements.items():
                event.data = event.data.replace(old, new)
        return self

    def shorten_digests(self) -> CastRecording:
        """Shorten ``@sha256:<64 hex>`` refs to ``@sha256:<12 hex>..`` for
        a readable recording. Merges close events first so a digest isn't
        split across two PTY read chunks."""
        self._merge_close_events()
        for event in self.events:
            event.data = _DIGEST_REF_RE.sub(r"@sha256:\1..", event.data)
        return self

    def _merge_close_events(self, threshold: float = 0.05) -> None:
        """Merge consecutive output events within *threshold* seconds.

        Compares each event to the previous *raw* event's timestamp, not the
        first event already folded into the current merge group -- a run of
        simulated keystrokes ``threshold``-apart (e.g. ``type_command``'s
        per-char events) must coalesce into one string so a sanitize()/
        shorten_digests() substring match isn't defeated by a match straddling
        a merge-group boundary. Comparing against the group's first timestamp
        instead would let cumulative drift exceed ``threshold`` after only a
        couple of characters.
        """
        if not self.events:
            return
        merged: list[CastEvent] = [self.events[0]]
        last_raw_ts = self.events[0].timestamp
        for event in self.events[1:]:
            prev = merged[-1]
            if event.event_type == prev.event_type and event.timestamp - last_raw_ts < threshold:
                prev.data += event.data
            else:
                merged.append(event)
            last_raw_ts = event.timestamp
        self.events = merged

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
        # wide enough to hold the *unshortened* digest before sanitize()
        # ever sees it -- shorten_digests() only rewrites captured bytes,
        # it cannot un-wrap a line the terminal already hard-wrapped.
        # `grim status`'s widest row (Kind/Name/Source/Pinned@sha256-64hex/
        # State) measures 154 cols for this demo's fixed repo path; 180
        # leaves headroom.
        width: int = 180,
        height: int = 24,
        prompt: str = "$ ",
        typing_delay: float = 0.04,
        inter_command_delay: float = 1.0,
        output_delay: float = 0.3,
        end_pause: float = 2.0,
    ):
        self.env = env
        self.cwd = cwd
        self.width = width
        self.height = height
        self.prompt = prompt
        self.typing_delay = typing_delay
        self.inter_command_delay = inter_command_delay
        self.output_delay = output_delay
        self.end_pause = end_pause
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

    def pause(self, seconds: float) -> None:
        self._clock += seconds

    def build(self, title: str = "", theme: dict[str, str] | None = None) -> CastRecording:
        # Add a final empty event so the player holds the last frame visible.
        events = list(self._events)
        self._clock += self.end_pause
        events.append(CastEvent(self._clock, "o", ""))
        return CastRecording(width=self.width, height=self.height, title=title, theme=theme, events=events)
