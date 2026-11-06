# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What a settings run leaves of the user's own files.

Every test here guards a defect that once cost a user part of a config file
grim only meant to add one key to. Each docstring names the failure in plain
words. Where formatting is the claim, the whole file is compared byte for byte
or line by line, never parsed. Everything runs at global scope with the vendor
variables redirected under ``tmp_path``; fixtures come from ``make_settings``.
Each "nothing was written" sits beside a run that writes.
"""

from __future__ import annotations

import difflib
import json
import os
import re
import tomllib
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

posix = pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks and modes")

JSON_ORIGINAL = '{\n    "zebra": 1,\n    "apple": {"b": 2, "a": 1},\n    "theme": "dark"\n}\n'
JSONC_ORIGINAL = (
    "// my copilot settings: keep this header\n"
    "{\n"
    "  \"zebra\": 1, // last letter first\n"
    "  /* block comment */\n"
    "  \"apple\": {\"b\": 2, \"a\": 1},\n"
    "  \"theme\": \"dark\"\n"
    "}\n"
)
TOML_ORIGINAL = (
    "# my codex config: keep this header\n"
    "zebra = 1 # last letter first\n"
    "\n"
    "[apple]\n"
    "b = 2\n"
    "a = 1\n"
    "\n"
    "[tui]\n"
    "# animations stay as they are\n"
    "animations = false\n"
)

# (client, filename, original bytes, package table, the same table after a pin roll)
FORMATS = {
    "json": ("claude", "settings.json", JSON_ORIGINAL, {"model": "sonnet", "permissions": {"deny": ["Bash(rm:*)"]}}, {"model": "opus", "permissions": {"deny": ["Bash(rm:*)"]}}),
    "jsonc": ("copilot", "settings.json", JSONC_ORIGINAL, {"model": "gpt-5", "deniedUrls": ["https://a.test"]}, {"model": "o3", "deniedUrls": ["https://a.test"]}),
    "toml": ("codex", "config.toml", TOML_ORIGINAL, {"model": "gpt-5", "sandbox_workspace_write": {"writable_roots": ["/a"]}}, {"model": "o3", "sandbox_workspace_write": {"writable_roots": ["/a"]}}),
}
SIMPLE = {"json": "claude", "jsonc": "copilot", "toml": "codex"}


class Scope:
    """A global scope with sandboxed vendor roots and a way to push packages."""

    def __init__(self, grim: GrimRunner, home: Path, tmp_path: Path, repo: str, clients: list[str]) -> None:
        self.grim, self.home, self.repo, self.clients = grim, home, repo, clients
        self.env, self.roots = settings_sandbox(tmp_path / "sandbox", grim.home)
        grim.env.update(self.env)
        self.tags: dict[str, int] = {}

    def package(self, name: str, tables: dict) -> PublishedArtifact:
        self.tags[name] = self.tags.get(name, 0) + 1
        return make_settings(f"{self.repo}/{name}", str(self.tags[name]), {"description": f"{name} defaults", **tables})

    def configure(self, packages: dict[str, PublishedArtifact]) -> None:
        write_config(
            self.home,
            settings={name: p.fq for name, p in packages.items()},
            options={"clients": json.dumps(self.clients)},
            experimental_settings=True,
        )
        self.grim.run("--global", "lock")

    def install(self, *flags: str, check: bool = True) -> CompletedProcess[str]:
        return self.grim.run("--global", "install", *flags, format="json", check=check)

    def uninstall(self, name: str, *flags: str, check: bool = True) -> CompletedProcess[str]:
        result = self.grim.run("--global", "uninstall", "settings", name, *flags, format="json", check=check)
        self.last_stderr = result.stderr
        return result

    def repin(self, old: PublishedArtifact, new: PublishedArtifact) -> None:
        lock = self.home / "grimoire.lock"
        text = lock.read_text(encoding="utf-8")
        assert old.digest in text
        lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8")

    def records(self) -> list[dict]:
        state = Path(self.grim.grim_home) / "state" / "global.json"
        if not state.exists():
            return []
        return [r for r in json.loads(state.read_text(encoding="utf-8"))["records"] if r["kind"] == "settings"]

    def pieces(self, client: str | None = None) -> dict[tuple[str, str, str | None], bool]:
        return {
            (r["name"], o["entry"], o.get("element")): o.get("adopted", False)
            for r in self.records()
            for o in r["outputs"]
            if "entry" in o and client in (None, o.get("client"))
        }


@pytest.fixture()
def scope(grim, grim_home: Path, tmp_path: Path, unique_repo: str):
    def make(clients: list[str]) -> Scope:
        return Scope(grim, grim_home, tmp_path, unique_repo, clients)

    return make


def _lines(text: str) -> list[str]:
    return text.splitlines(keepends=True)


def _norm(text: str) -> list[str]:
    """Lines, each without a trailing comma: adding a member after the last one adds that comma."""
    return [re.sub(r",(\s*(//.*)?)$", r"\1", line.rstrip()) + "\n" for line in text.splitlines()]


def _only_additions(before: str, after: str) -> list[str]:
    """The lines `after` adds; fails if any line of `before` is lost or moved."""
    diff = list(difflib.unified_diff(_norm(before), _norm(after), n=0))
    removed = [line for line in diff if line.startswith("-") and not line.startswith("---")]
    assert removed == [], f"a line of the user's file changed or went:\n{''.join(diff)}"
    return [line[1:] for line in diff if line.startswith("+") and not line.startswith("+++")]


# ── Comments and key order ───────────────────────────────────────────────


@pytest.mark.parametrize("fmt", FORMATS)
def test_comments_order_and_layout_survive_every_edit(scope, fmt: str) -> None:
    """A user's comments, key order and indentation survive an add, a change and a removal.

    Once the config was parsed and written back whole: comments dropped, keys
    re-sorted alphabetically, the file reflowed. Now each edit may only add or
    swap its own line; the removal brings back the original bytes.
    """
    client, filename, original, first_table, second_table = FORMATS[fmt]
    s = scope([client])
    path = s.roots[client] / filename
    path.write_text(original, encoding="utf-8")
    first, second = s.package("team", {client: first_table}), s.package("team", {client: second_table})
    s.configure({"team": first})

    s.install()
    added = path.read_text(encoding="utf-8")
    new_lines = _only_additions(original, added)
    assert new_lines, "positive control: the install wrote something"
    assert any("model" in line for line in new_lines)
    order = ("zebra", "apple", "tui") if fmt == "toml" else ("zebra", "apple", "theme")
    assert [added.index(key) for key in order] == sorted(added.index(key) for key in order), "key order"
    for marker in ("keep this header", "last letter first", "block comment", "animations stay"):
        assert (marker in added) == (marker in original), marker

    s.repin(first, second)
    s.install()
    changed = path.read_text(encoding="utf-8")
    swapped = list(difflib.unified_diff(_lines(added), _lines(changed), n=0))
    gone = [line for line in swapped if line.startswith("-") and not line.startswith("---")]
    assert len(gone) == 1 and "model" in gone[0], swapped

    s.uninstall("team")
    assert path.read_text(encoding="utf-8") == original, "removal restores the user's bytes"


def test_toml_container_header_and_comment_survive(scope) -> None:
    """A hand-written `[tui]` header and the comment under it stay when a key is added and removed.

    Once the splice marked the table "implicit", so the next write printed it
    without its header and the comment attached to it went with it.
    """
    s = scope(["codex"])
    path = s.roots["codex"] / "config.toml"
    original = "[tui]\n# animations stay as they are\nanimations = false\n"
    path.write_text(original, encoding="utf-8")
    team = s.package("team", {"codex": {"tui": {"auto_recap": True}}})
    s.configure({"team": team})

    s.install()

    text = path.read_text(encoding="utf-8")
    assert text.count("[tui]") == 1 and "# animations stay as they are" in text
    assert tomllib.loads(text) == {"tui": {"animations": False, "auto_recap": True}}
    _only_additions(original, text)

    s.uninstall("team")
    assert path.read_text(encoding="utf-8") == original


@pytest.mark.parametrize("fmt", ["json", "jsonc"])
def test_set_element_is_spliced_not_reserialised(scope, fmt: str) -> None:
    """Adding one element to a user's array leaves the array's own layout and comments.

    Once the managed list entry was added by rewriting the whole document, which
    alphabetised keys and dropped comments next to the array.
    """
    client = SIMPLE[fmt]
    key = ("permissions", "deny") if fmt == "json" else ("deniedUrls",)
    element, user = ("Bash(rm:*)", "Bash(curl:*)") if fmt == "json" else ("https://a.test", "https://u.test")
    s = scope([client])
    path = s.roots[client] / "settings.json"
    inner = f'[\n      "{user}", // mine\n      "keep"\n    ]'
    body = f'"permissions": {{"deny": {inner}}}' if fmt == "json" else f'"deniedUrls": {inner}'
    original = "// head\n{\n  \"zebra\": 1,\n  " + body + ",\n  \"apple\": 2\n}\n"
    if fmt == "json":
        original = original.replace(" // mine", "").replace("// head\n", "")
    path.write_text(original, encoding="utf-8")
    table = nest_table(key, [element])
    s.configure({"team": s.package("team", {client: table})})

    s.install()

    text = path.read_text(encoding="utf-8")
    assert element in text
    for marker in ("zebra", "apple", "keep"):
        assert marker in text
    assert text.index("zebra") < text.index("apple")
    assert ("// head" in text) == ("// head" in original)
    assert ("// mine" in text) == ("// mine" in original)


def nest_table(path: tuple[str, ...], leaf: object) -> dict:
    for key in reversed(path):
        leaf = {key: leaf}
    return leaf  # type: ignore[return-value]


# ── No competing file ────────────────────────────────────────────────────


def test_existing_jsonc_is_edited_in_place(scope) -> None:
    """A user's `opencode.jsonc` gets the key; no `opencode.json` appears beside it.

    Once grim wrote a fresh `opencode.json` next to the user's `.jsonc`, so
    OpenCode read two files and the user's own was never updated.
    """
    s = scope(["opencode"])
    root = Path(s.grim.home) / ".config" / "opencode"
    root.mkdir(parents=True)
    jsonc = root / "opencode.jsonc"
    original = '// mine\n{\n  "theme": "dark" // keep\n}\n'
    jsonc.write_text(original, encoding="utf-8")
    s.configure({"team": s.package("team", {"opencode": {"model": "anthropic/sonnet"}})})

    s.install()

    text = jsonc.read_text(encoding="utf-8")
    assert "anthropic/sonnet" in text and "// mine" in text and "// keep" in text
    assert not (root / "opencode.json").exists(), "no competing spelling"
    _only_additions(original, text)

    # Positive control: with no file at all, the plain spelling is written.
    jsonc.unlink()
    s.install()
    assert (root / "opencode.json").is_file()
    assert not jsonc.exists()


# ── Symlinked dotfiles ───────────────────────────────────────────────────


@posix
@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize("depth", [1, 2], ids=["link", "chain"])
def test_symlinked_config_stays_a_symlink(scope, tmp_path: Path, fmt: str, depth: int) -> None:
    """A config that is a link (or a chain of links) into a dotfiles repo is edited there.

    Once the write replaced the link with a regular file, silently detaching the
    user's config from their dotfiles repository. Both the install and the
    uninstall write through the link, and every link in the chain stays a link.
    """
    client, filename, original, first_table, _ = FORMATS[fmt]
    s = scope([client])
    dotfiles = tmp_path / "dotfiles"
    dotfiles.mkdir()
    real = dotfiles / "real"
    real.write_text(original, encoding="utf-8")
    target = real
    chain = []
    for index in range(depth):
        link = dotfiles / f"hop{index}"
        link.symlink_to(target)
        chain.append(link)
        target = link
    entry = s.roots[client] / filename
    entry.symlink_to(target)
    s.configure({"team": s.package("team", {client: first_table})})

    s.install()

    assert entry.is_symlink() and all(link.is_symlink() for link in chain)
    assert "model" in real.read_text(encoding="utf-8")
    _only_additions(original, real.read_text(encoding="utf-8"))

    s.uninstall("team")

    assert entry.is_symlink() and all(link.is_symlink() for link in chain)
    assert real.read_text(encoding="utf-8") == original


# ── Uninstall and what is not grim's ─────────────────────────────────────


def report_stderr(s: Scope) -> str:
    return s.last_stderr


def _claude_file(s: Scope) -> Path:
    return s.roots["claude"] / "settings.json"


def test_uninstall_leaves_a_modified_value_until_forced(scope) -> None:
    """A value the user changed after install is not deleted by uninstall.

    Once uninstall spliced out a managed entry without comparing it to what was
    recorded, discarding the user's own edit. It now refuses, names the key,
    and removes it only under `--force`.
    """
    s = scope(["claude"])
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet", "language": "en"}})})
    s.install()
    target = _claude_file(s)
    target.write_text(json.dumps({"model": "mine", "language": "en"}), encoding="utf-8")
    held = target.read_bytes()

    refused = s.uninstall("team", check=False)

    assert refused.returncode == 65, refused.stderr
    assert json.loads(refused.stdout)["error"]["reason"] == "modified"
    assert target.read_bytes() == held, "one refusal moves nothing"
    assert ("team", "/language", None) in s.pieces()

    s.uninstall("team", "--force")
    assert json.loads(target.read_text(encoding="utf-8")) == {}
    assert s.pieces() == {}


def test_uninstall_leaves_an_adopted_value_and_says_so(scope) -> None:
    """A value that was already in the file, and merely matched, is never deleted.

    Once the install adopted an identical entry (writing no bytes) and the
    uninstall then deleted something grim never wrote. It is left, and reported
    in `abandoned_entries`.
    """
    s = scope(["claude"])
    target = _claude_file(s)
    target.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet", "language": "en"}})})
    s.install()
    assert s.pieces()[("team", "/model", None)] is True
    assert s.pieces()[("team", "/language", None)] is False

    report = json.loads(s.uninstall("team").stdout)

    assert json.loads(target.read_text(encoding="utf-8")) == {"model": "sonnet"}
    assert [(Path(row["path"]).resolve(), row["pointer"]) for row in report["abandoned_entries"]] == [
        (target.resolve(), "/model")
    ]
    assert s.pieces() == {}


def test_adopted_value_stays_adopted_and_reachable(scope) -> None:
    """A re-install that rebuilds the record keeps an adopted value adopted, and `--force` still reaches it.

    Once a widened client set or a pin roll rebuilt the record without the
    `adopted` flag, so the next uninstall deleted the user's own value; and once
    the record was gone, nothing could remove a value the user then wanted out.
    """
    s = scope(["claude", "codex"])
    target = _claude_file(s)
    target.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
    first = s.package("team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    second = s.package("team", {"claude": {"model": "sonnet", "language": "en"}, "codex": {"model": "gpt"}})
    s.configure({"team": first})
    s.grim.run("--global", "install", "--client", "claude")
    assert s.pieces("claude")[("team", "/model", None)] is True

    s.repin(first, second)
    s.install()

    assert s.pieces("claude")[("team", "/model", None)] is True, "a rebuilt record keeps the flag"
    assert json.loads(target.read_text(encoding="utf-8")) == {"model": "sonnet", "language": "en"}

    # An intact adopted value is left even under `--force`, and named so the user can take it out.
    report = json.loads(s.uninstall("team", "--force").stdout)
    assert json.loads(target.read_text(encoding="utf-8")) == {"model": "sonnet"}
    assert [(Path(row["path"]).resolve(), row["pointer"]) for row in report["abandoned_entries"]] == [
        (target.resolve(), "/model")
    ]
    assert s.pieces() == {}


def test_drifted_adopted_value_refuses_and_force_removes_it(scope) -> None:
    """An adopted value the user then changed is refused by uninstall, and `--force` takes it out.

    Once the record of an adopted entry could vanish while the member stayed
    live, leaving no command able to remove it; `--force` is the one explicit
    way out, and without it the user's changed value is never discarded.
    """
    s = scope(["claude"])
    target = _claude_file(s)
    target.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet"}})})
    s.install()
    assert s.pieces()[("team", "/model", None)] is True
    target.write_text(json.dumps({"model": "mine"}), encoding="utf-8")
    held = target.read_bytes()

    refused = s.uninstall("team", check=False)

    assert refused.returncode == 65, refused.stderr
    assert target.read_bytes() == held
    assert ("team", "/model", None) in s.pieces()
    s.uninstall("team", "--force")
    assert json.loads(target.read_text(encoding="utf-8")) == {}
    assert s.pieces() == {}


# ── A relocated root ─────────────────────────────────────────────────────


def test_relocated_root_strands_nothing_unreported(scope, tmp_path: Path) -> None:
    """A shell move of a vendor's config directory warns about the old copy, never losing it in silence.

    A record stores an anchor, and after this move the anchor resolves to the new
    root, so no later command can reach the old file. The ADR promises a name
    only when the record can still resolve it, which the settings-derived move
    below can. Here the promise is a warning on the run that finds the move.
    """
    s = scope(["claude"])
    old = s.roots["claude"] / "settings.json"
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet"}})})
    assert "old root" not in s.install().stderr, "positive control: nothing moved yet"
    assert json.loads(old.read_text(encoding="utf-8")) == {"model": "sonnet"}
    new_root = tmp_path / "moved-claude"
    new_root.mkdir()
    s.grim.env["CLAUDE_CONFIG_DIR"] = str(new_root)

    moved = s.install()

    new = new_root / "settings.json"
    assert json.loads(new.read_text(encoding="utf-8")) == {"model": "sonnet"}, "the value lands at the new root"
    assert json.loads(old.read_text(encoding="utf-8")) == {"model": "sonnet"}, "a copy at the old root is not deleted"
    assert "old root" in moved.stderr, "the run that found the move says a copy may remain"
    assert "old root" not in s.install().stderr, "the record now holds the new root, so the next run is quiet"

    report = json.loads(s.uninstall("team").stdout)

    assert report["abandoned_entries"] == [], "no name can be given for a file the record does not reach"
    assert "model" not in json.loads(new.read_text(encoding="utf-8"))
    assert json.loads(old.read_text(encoding="utf-8")) == {"model": "sonnet"}, "the old copy is the user's to remove"


def test_relocated_root_warns_when_uninstall_finds_the_move(scope, tmp_path: Path) -> None:
    """An uninstall that is the first run after a shell move warns too: the record is about to go."""
    s = scope(["claude"])
    old = s.roots["claude"] / "settings.json"
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet"}})})
    s.install()
    new_root = tmp_path / "moved-claude"
    new_root.mkdir()
    s.grim.env["CLAUDE_CONFIG_DIR"] = str(new_root)

    report = json.loads(s.uninstall("team").stdout)

    assert "old root" in report_stderr(s)
    assert report["abandoned_entries"] == []
    assert s.records() == [], "positive control: the uninstall went through"
    assert json.loads(old.read_text(encoding="utf-8")) == {"model": "sonnet"}


def test_settings_derived_root_move_names_the_old_copy(scope, tmp_path: Path) -> None:
    """A root moved by Claude's own settings `env` still resolves, so the old copy is named on both runs."""
    s = scope(["claude"])
    old = s.roots["claude"] / "settings.json"
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet"}})})
    s.install()
    new_root = tmp_path / "moved-claude"
    new_root.mkdir()
    document = json.loads(old.read_text(encoding="utf-8"))
    document["env"] = {"CLAUDE_CONFIG_DIR": str(new_root)}
    old.write_text(json.dumps(document), encoding="utf-8")

    moved = s.install()

    new = new_root / "settings.json"
    assert json.loads(new.read_text(encoding="utf-8")) == {"model": "sonnet"}, "the value lands at the new root"
    assert str(old) in moved.stderr and "left in place" in moved.stderr, moved.stderr
    assert "old root" not in moved.stderr, "a named copy needs no guess"
    assert "left in place" not in s.install().stderr, "the record now holds the new root, so the next run is quiet"

    report = json.loads(s.uninstall("team").stdout)

    named = [(Path(row["path"]).resolve(), row["pointer"]) for row in report["abandoned_entries"]]
    assert named == [(old.resolve(), "/model")]
    assert "model" not in json.loads(new.read_text(encoding="utf-8"))
    assert json.loads(old.read_text(encoding="utf-8"))["model"] == "sonnet", "the old copy is the user's to remove"


# ── A run that stops partway ─────────────────────────────────────────────


@posix
@pytest.mark.skipif(not hasattr(os, "geteuid") or os.geteuid() == 0, reason="needs a non-root user to deny a write")
def test_partial_run_records_what_it_wrote(scope) -> None:
    """When a later client cannot be written, the earlier one is still recorded.

    Once a refusal on a later client left an earlier client's file carrying a
    grim-written value that nothing recorded, so uninstall could never find it
    and every reinstall re-refused it as foreign.
    """
    s = scope(["claude", "codex"])
    s.configure({"team": s.package("team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})})
    codex_root = s.roots["codex"]
    codex_root.chmod(0o500)
    try:
        failed = s.install(check=False)
    finally:
        codex_root.chmod(0o700)

    assert failed.returncode != 0
    written = json.loads(_claude_file(s).read_text(encoding="utf-8")) if _claude_file(s).exists() else {}
    recorded = {(name, entry) for (name, entry, _) in s.pieces()}
    if written == {"model": "sonnet"}:
        assert ("team", "/model") in recorded, "a value grim wrote must be recorded"
    assert not (codex_root / "config.toml").exists()

    # The retry converges: both files carry the value, recorded and not adopted.
    s.install()
    assert json.loads(_claude_file(s).read_text(encoding="utf-8")) == {"model": "sonnet"}
    assert 'model = "gpt"' in (codex_root / "config.toml").read_text(encoding="utf-8")
    assert set(s.pieces().values()) == {False}, "grim's own earlier write is not adopted"
    s.uninstall("team")
    assert json.loads(_claude_file(s).read_text(encoding="utf-8")) == {}


# ── A key is escaped ─────────────────────────────────────────────────────

AWKWARD = 'a"b\\c d'


@pytest.mark.parametrize("fmt", ["json", "jsonc", "toml"])
def test_awkward_key_is_escaped(scope, fmt: str) -> None:
    """A map entry whose name holds a quote and a backslash lands as exactly that key.

    Once member keys were spliced into JSON unescaped, so a quote in a name
    ended the key early and the whole config stopped parsing.
    """
    client, filename = SIMPLE[fmt], FORMATS[fmt][1]
    table = {
        "json": {"extraKnownMarketplaces": {AWKWARD: {"source": {"source": "github", "repo": "acme/tools"}}}},
        "jsonc": {"extraKnownMarketplaces": {AWKWARD: {"source": "https://example.com/a"}}},
        "toml": {"marketplaces": {AWKWARD: {"source": "https://example.com/a", "source_type": "git"}}},
    }[fmt]
    s = scope([client])
    path = s.roots[client] / filename
    original = {"json": '{\n  "theme": "dark"\n}\n', "jsonc": '// mine\n{\n  "theme": "dark"\n}\n', "toml": "# mine\ntheme = 'dark'\n"}[fmt]
    path.write_text(original, encoding="utf-8")
    s.configure({"team": s.package("team", {client: table})})

    s.install()

    text = path.read_text(encoding="utf-8")
    if fmt == "toml":
        parsed = tomllib.loads(text)
    else:
        parsed = json.loads("\n".join(line for line in text.splitlines() if not line.startswith("//")))
    container = parsed["marketplaces" if fmt == "toml" else "extraKnownMarketplaces"]
    assert list(container) == [AWKWARD]
    assert parsed["theme"] == "dark"

    s.uninstall("team")
    assert path.read_text(encoding="utf-8") == original


# ── A repointed OPENCODE_CONFIG ──────────────────────────────────────────


def test_repointed_opencode_config_edits_the_recorded_file(scope, tmp_path: Path) -> None:
    """After `OPENCODE_CONFIG` moves, grim edits the file it wrote, not the one the variable now names.

    Once the removal recomputed the path from the current environment, so a
    repointed variable made grim edit a file it had never written.
    """
    s = scope(["opencode"])
    first = tmp_path / "first.json"
    second = tmp_path / "second.json"
    first.write_text("{}\n", encoding="utf-8")
    mine = '{\n  "model": "mine/own"\n}\n'
    second.write_text(mine, encoding="utf-8")
    s.grim.env["OPENCODE_CONFIG"] = str(first)
    team = s.package("team", {"opencode": {"model": "anthropic/sonnet"}})
    s.configure({"team": team})
    s.install()
    assert json.loads(first.read_text(encoding="utf-8")) == {"model": "anthropic/sonnet"}

    s.grim.env["OPENCODE_CONFIG"] = str(second)
    other = s.package("team", {"opencode": {"small_model": "anthropic/haiku"}})
    s.repin(team, other)
    held = first.read_bytes()

    result = s.install()

    assert json.loads(second.read_text(encoding="utf-8"))["model"] == "mine/own", "the user's own key is not removed"
    assert first.read_bytes() == held, "a file this run does not resolve is not edited from a recomputed path"
    assert "set aside" in result.stderr
    assert json.loads(second.read_text(encoding="utf-8"))["small_model"] == "anthropic/haiku", "positive control: the run wrote"

    # Pointed back, the recorded file is the one edited.
    s.grim.env["OPENCODE_CONFIG"] = str(first)
    s.install()
    assert "model" not in json.loads(first.read_text(encoding="utf-8")), "the recorded file lost the dropped key"


def test_repointed_run_holds_the_pin_only_for_a_dropped_value(scope, tmp_path: Path) -> None:
    """The pin waits for a dropped value in another file, and for nothing else.

    Moved at once, no later run would see a change, and the dropped value would
    stay in the recorded file for good. A value the lock still declares has
    nothing to wait for, so its pin moves as usual.
    """
    s = scope(["opencode"])
    first = tmp_path / "first.json"
    second = tmp_path / "second.json"
    first.write_text("{}\n", encoding="utf-8")
    second.write_text("{}\n", encoding="utf-8")
    s.grim.env["OPENCODE_CONFIG"] = str(first)
    team = s.package("team", {"opencode": {"model": "anthropic/sonnet"}})
    s.configure({"team": team})
    s.install()
    pin = lambda: s.records()[0]["pinned"]  # noqa: E731
    installed = pin()

    # The same value under a new pin: a run from the other file drops nothing.
    s.grim.env["OPENCODE_CONFIG"] = str(second)
    same = s.package("team", {"description": "reworded", "opencode": {"model": "anthropic/sonnet"}})
    s.repin(team, same)
    s.install()
    assert pin() != installed, "a value the lock still declares holds no pin back"

    dropped = pin()
    other = s.package("team", {"opencode": {"small_model": "anthropic/haiku"}})
    s.repin(same, other)
    s.grim.env["OPENCODE_CONFIG"] = str(first)
    s.install()
    assert "model" in json.loads(second.read_text(encoding="utf-8")), "the dropped value sits in the other file"
    assert pin() == dropped, "the pin waits for a run that reaches the dropped value"

    repeat = s.install()
    assert json.loads(repeat.stdout)["settings_changes"] == [], "a held pin rewrites nothing on a repeat run"
    assert pin() == dropped

    s.grim.env["OPENCODE_CONFIG"] = str(second)
    s.install()
    assert "model" not in json.loads(second.read_text(encoding="utf-8")), "the recorded file lost the dropped key"
    assert pin() != dropped, "with the value judged the pin moves"
