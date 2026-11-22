# /// script
# requires-python = ">=3.10"
# ///
"""PreToolUse hook (Bash): enforce pre-commit verification gate.

Fires only when a Bash tool call contains a git commit command.
Blocks the commit unless `task verify` was run recently (5-minute TTL).
Covers this repository's worktrees only; commits in other repositories pass.
Uses deny (hard block), not additionalContext (ignorable hint).
"""

import sys

sys.path.insert(0, str(__import__("pathlib").Path(__file__).parent))

import re
import shlex
from pathlib import Path

import hook_utils

_GIT_COMMIT_RE = re.compile(r"\bgit\s+commit\b")
_LOOSE_COMMIT_RE = re.compile(r"\bgit\b[^;&|\n]*\bcommit\b")
_GIT_DASH_C_RE = re.compile(r"\bgit\s+-C\s+(?P<q>[\"']?)(?P<path>[^\s\"';&|]+)(?P=q)")
_CD_RE = re.compile(r"(?:^|&&|;)\s*cd\s+(?P<q>[\"']?)(?P<path>[^\s\"';&|]+)(?P=q)")
# Git options that take their value as the next word; skipping the value is
# what finds the subcommand behind them.
_GIT_VALUE_OPTIONS = frozenset(
    {
        "-C",
        "-c",
        "--git-dir",
        "--work-tree",
        "--namespace",
        "--super-prefix",
        "--config-env",
        "--attr-source",
    }
)
# Words that can point a `git` call at another repository or run other code.
_GIT_REDIRECTING = (
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--exec-path",
)
_SHELL_PUNCTUATION = frozenset("();<>|&")


# ---------------------------------------------------------------------------
# Pure logic functions (testable)
# ---------------------------------------------------------------------------


def _simple_commands(command: str) -> list[tuple[str, list[str]]] | None:
    """Split shell text into `(operator before, argv)` simple commands.

    Returns None when the text cannot be tokenised (an unclosed quote).
    """
    text = command.replace("\\\n", " ").replace("\n", " ; ")
    lexer = shlex.shlex(text, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        tokens = list(lexer)
    except ValueError:
        return None
    commands: list[tuple[str, list[str]]] = [("", [])]
    for token in tokens:
        if token and set(token) <= _SHELL_PUNCTUATION:
            commands.append((token, []))
        else:
            commands[-1][1].append(token)
    return commands


def _git_subcommand(args: list[str]) -> str | None:
    """Return the first word after `git` that is neither an option nor its value."""
    skip = False
    for arg in args:
        if skip:
            skip = False
        elif arg in _GIT_VALUE_OPTIONS:
            skip = True
        elif not arg.startswith("-"):
            return arg
    return None


def is_git_commit(command: str, _depth: int = 0) -> bool:
    """Return True if the shell command contains a git commit invocation.

    Looks behind git's own options and shell quoting, and into quoted words
    that hold a command line (`bash -c "..."`). Text that cannot be tokenised
    counts as a commit when `git` and the subcommand share a line.
    """
    if _GIT_COMMIT_RE.search(command):
        return True
    commands = _simple_commands(command)
    if commands is None:
        return bool(_LOOSE_COMMIT_RE.search(command))
    for _, argv in commands:
        for i, word in enumerate(argv):
            if word == "git" or word.endswith("/git"):
                if _git_subcommand(argv[i + 1 :]) == "commit":
                    return True
            elif _depth < 2 and " " in word and is_git_commit(word, _depth + 1):
                return True
    return False


def resolve_commit_project_dir(command: str, fallback: str) -> str:
    """Return the root of the git worktree the commit targets.

    `task verify` stamps commit-verified under the worktree it runs in, so the
    gate must read the stamp of the tree being committed — not unconditionally
    CLAUDE_PROJECT_DIR. Extracts `git -C <path>` or a leading `cd <path>` from
    the command and walks up to the containing worktree root (a directory with
    a `.git` entry). Falls back to `fallback` when the command carries no
    directory hint or the hint is not inside a git worktree.
    """
    # Strip quoted segments first so text inside -m "..." (e.g. a commit
    # message mentioning `git -C x` or `cd x`) cannot steer the resolution.
    stripped = re.sub(r"'[^']*'|\"[^\"]*\"", "", command)
    m = _GIT_DASH_C_RE.search(stripped) or _CD_RE.search(stripped)
    if not m:
        return fallback
    try:
        start = Path(m.group("path")).resolve()
    except OSError:
        return fallback
    for candidate in (start, *start.parents):
        if (candidate / ".git").exists():
            return str(candidate)
    return fallback


def _worktree_root(start: Path) -> Path | None:
    for candidate in (start, *start.parents):
        if (candidate / ".git").exists():
            return candidate
    return None


def _git_common_dir(root: Path) -> Path:
    """Return the resolved git common directory of the worktree at `root`.

    Reads `.git` directly (no subprocess). Raises OSError or ValueError when
    the `.git` file is unreadable or malformed.
    """
    dot_git = root / ".git"
    if dot_git.is_dir():
        return dot_git.resolve()
    first = dot_git.read_text().splitlines()[0]
    if not first.startswith("gitdir:"):
        raise ValueError("malformed .git file")
    gitdir = (root / first[len("gitdir:") :].strip()).resolve()
    commondir = gitdir / "commondir"
    if commondir.exists():
        return (gitdir / commondir.read_text().strip()).resolve()
    return gitdir


def _foreign_hints(commands: list[tuple[str, list[str]]]) -> list[str] | None:
    """Return the directory hints of the two shapes that leave no doubt.

    Shape one: every simple command is `git -C <path> ...`, joined by `&&` or
    `;`. Shape two: `cd <path>` first, then only `git ...`, all joined by `&&`
    so a failed `cd` stops the chain. Anything else returns None.
    """
    operators = [op for op, _ in commands[1:]]
    argvs = [argv for _, argv in commands]
    if any(not argv for argv in argvs):
        return None

    def redirects(words: list[str]) -> bool:
        return any(
            w == r or w.startswith(r + "=") for w in words for r in _GIT_REDIRECTING
        )

    if all(
        len(a) >= 4 and a[:2] == ["git", "-C"] and not redirects(a[3:]) for a in argvs
    ):
        return [a[2] for a in argvs] if set(operators) <= {"&&", ";"} else None
    first, rest = argvs[0], argvs[1:]
    chained = len(first) == 2 and first[0] == "cd" and rest and set(operators) == {"&&"}
    if chained and all(a[0] == "git" and not redirects(a[1:]) for a in rest):
        return [first[1]]
    return None


def targets_foreign_repository(command: str, project_dir: str) -> bool:
    """Return True only when the commit surely targets another repository.

    Only the two shapes of `_foreign_hints` count, with literal absolute paths
    of existing directories that share one worktree root. Substitution,
    variables, several lines and every other command word are doubt and keep
    the guard on. The root must have another git common directory.
    """
    if any(mark in command for mark in ("\n", "`", "$")):
        return False
    commands = _simple_commands(command)
    hints = _foreign_hints(commands) if commands else None
    if not hints:
        return False
    if any(
        not Path(h).is_absolute() or set(h) & set("~*?[]{}") or not Path(h).is_dir()
        for h in hints
    ):
        return False
    try:
        roots = {_worktree_root(Path(h).resolve()) for h in hints}
        own = _worktree_root(Path(project_dir).resolve())
        if len(roots) != 1 or None in roots or own is None:
            return False
        return _git_common_dir(roots.pop()) != _git_common_dir(own)
    except (OSError, ValueError, IndexError):
        return False


def detect_project_tools(project_dir: str) -> list[str]:
    """Detect available project tools by inspecting well-known files.

    Returns a list of tool-name strings such as "cargo-test", "ruff", "go-vet".
    """
    root = Path(project_dir)
    tools: list[str] = []

    # --- Rust ---
    if (root / "Cargo.toml").exists():
        tools.extend(["cargo-test", "cargo-clippy", "cargo-fmt"])

    # --- TypeScript / JavaScript ---
    pkg_json = root / "package.json"
    if pkg_json.exists():
        try:
            pkg_text = pkg_json.read_text()
        except OSError:
            pkg_text = ""

        # Determine package manager
        if (root / "pnpm-lock.yaml").exists():
            pkg_mgr = "pnpm"
        elif (root / "package-lock.json").exists():
            pkg_mgr = "npm"
        else:
            pkg_mgr = "pnpm"

        if '"lint"' in pkg_text:
            tools.append(f"lint({pkg_mgr})")
        if '"test"' in pkg_text:
            tools.append(f"test({pkg_mgr})")
        # Match any of the three typecheck script name variants
        if re.search(r'"typecheck|"tsc|"type-check"', pkg_text):
            tools.append(f"typecheck({pkg_mgr})")

        if (root / "biome.json").exists() or (root / "biome.jsonc").exists():
            tools.append("biome")

    # --- Python ---
    pyproject = root / "pyproject.toml"
    if pyproject.exists():
        try:
            py_text = pyproject.read_text()
        except OSError:
            py_text = ""

        if "ruff" in py_text:
            tools.append("ruff")
        if "pytest" in py_text:
            tools.append("pytest")
        if "mypy" in py_text:
            tools.append("mypy")

    # --- Go ---
    if (root / "go.mod").exists():
        tools.extend(["go-test", "go-vet"])
        # golangci-lint: available in PATH or configured locally
        import shutil

        if shutil.which("golangci-lint") or (root / ".golangci.yml").exists():
            tools.append("golangci-lint")

    return tools


def build_deny_reason(detected_tools: list[str], state_dir: str) -> str:
    """Return the deny reason explaining how to pass the verification gate."""
    tools_str = (
        " ".join(detected_tools) if detected_tools else "none detected - check manually"
    )
    return (
        "BLOCKED: Cannot commit without passing verification.\n\n"
        "Run `task verify` first (format, clippy, lint, license, build, tests).\n"
        f"Detected tools: {tools_str}\n\n"
        "After verification passes, mark it complete:\n"
        f"  echo $(date +%s) > {state_dir}/commit-verified\n\n"
        "Then retry the commit."
    )


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main() -> None:
    data = hook_utils.read_input()

    tool_name: str = data.get("tool_name", "")
    if tool_name != "Bash":
        sys.exit(0)

    command: str = data.get("tool_input", {}).get("command", "")
    if not is_git_commit(command):
        sys.exit(0)

    project_dir = hook_utils.get_project_dir()
    if not project_dir:
        sys.exit(0)
    if targets_foreign_repository(command, project_dir):
        sys.exit(0)
    project_dir = resolve_commit_project_dir(command, project_dir)
    state = hook_utils.StateManager(project_dir)

    if state.is_recently_verified():
        sys.exit(0)

    tools = detect_project_tools(project_dir)
    state_dir_str = str(state.state_dir)
    reason = build_deny_reason(tools, state_dir_str)

    hook_utils.output_json(hook_utils.deny(reason))
    sys.exit(0)


if __name__ == "__main__":
    main()
