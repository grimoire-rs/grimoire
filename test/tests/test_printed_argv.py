"""`printed_argv` reads back every word form grim prints for a Windows shell."""

from __future__ import annotations

import sys

import pytest

from src.helpers import printed_argv


@pytest.mark.parametrize(
    ("command", "words"),
    [
        (r"grim --config C:\a\grimoire.toml uninstall", ["grim", "--config", r"C:\a\grimoire.toml", "uninstall"]),
        (r'grim --config "C:\Program Files\grimoire.toml"', ["grim", "--config", r"C:\Program Files\grimoire.toml"]),
        (r"grim --config 'C:\it''s $x\grimoire.toml'", ["grim", "--config", r"C:\it's $x\grimoire.toml"]),
        ("grim --config 'C:\\a\u2019\u2019b $x\\'", ["grim", "--config", "C:\\a\u2019b $x\\"]),
        ("grim --config 'ends with quote'''", ["grim", "--config", "ends with quote'"]),
    ],
)
def test_windows_words_read_back(monkeypatch: pytest.MonkeyPatch, command: str, words: list[str]) -> None:
    monkeypatch.setattr(sys, "platform", "win32")
    assert printed_argv(command) == words


def test_posix_words_keep_shell_splitting(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sys, "platform", "linux")
    assert printed_argv("grim --config '/a b/grimoire.toml' update") == ["grim", "--config", "/a b/grimoire.toml", "update"]
