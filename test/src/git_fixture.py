"""Deterministic local git upstreams for the grim-mirror acceptance tests.

Every repository is built with a pinned identity and pinned dates, no global
or system git config, and no signing, so the same calls yield the same commit
ids on every host. Nothing here touches the network: upstreams are reached
through ``file://`` URLs.
"""

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

#: The full MIT text the licence matcher recognises; the default `LICENSE` of the builders below.
MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates/grim-mirror/licenses/MIT.txt").read_text(encoding="utf-8")

#: Date of the first revision of every fixture repository; later calls pass their own.
BASE_DATE = "2026-01-01T00:00:00+00:00"

_IDENTITY = {
    "GIT_AUTHOR_NAME": "Fixture Author",
    "GIT_AUTHOR_EMAIL": "author@example.invalid",
    "GIT_COMMITTER_NAME": "Fixture Author",
    "GIT_COMMITTER_EMAIL": "author@example.invalid",
}


def _env(date: str | None = None) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(_IDENTITY)
    env["GIT_CONFIG_GLOBAL"] = os.devnull
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    if date is not None:
        env["GIT_AUTHOR_DATE"] = date
        env["GIT_COMMITTER_DATE"] = date
    return env


@dataclass
class GitRepo:
    """A local, non-bare git repository with a linear history."""

    path: Path
    _tick: int = field(default=0, repr=False)

    @classmethod
    def init(cls, path: Path) -> GitRepo:
        """Create an empty repository on branch ``main`` at ``path``."""
        path.mkdir(parents=True, exist_ok=True)
        repo = cls(path)
        repo.git("init", "--quiet", "--initial-branch=main")
        return repo

    def git(self, *args: str, date: str | None = None) -> str:
        """Run git in the repository and return stdout; raises on failure."""
        proc = subprocess.run(
            ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
            cwd=self.path,
            env=_env(date),
            capture_output=True,
            text=True,
            check=False,
        )
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
        return proc.stdout

    def write(self, rel: str, content: str | bytes) -> Path:
        """Write a file (creating parents) without recording it."""
        target = self.path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8", newline="\n")
        return target

    def remove(self, rel: str) -> None:
        """Delete a tracked file from the working tree and the index."""
        self.git("rm", "--quiet", "-r", "--", rel)

    def record(
        self,
        message: str,
        files: dict[str, str | bytes] | None = None,
        date: str | None = None,
    ) -> str:
        """Write ``files``, stage everything, record a revision, return its id.

        Without ``date`` each revision is one minute after the previous one, so
        history order is stable. An empty revision is allowed.
        """
        for rel, content in (files or {}).items():
            self.write(rel, content)
        if date is None:
            date = f"2026-01-01T00:{self._tick // 60:02d}:{self._tick % 60:02d}+00:00"
            self._tick += 1
        self.git("add", "--all")
        self.git("commit", "--quiet", "--allow-empty", "-m", message, date=date)
        return self.head()

    def head(self) -> str:
        return self.git("rev-parse", "HEAD").strip()

    def tag(self, name: str, rev: str = "HEAD") -> None:
        """Create a lightweight tag."""
        self.git("tag", name, rev)

    def tag_annotated(self, name: str, message: str = "release", rev: str = "HEAD", date: str = BASE_DATE) -> None:
        """Create an annotated tag with a pinned tagger date."""
        self.git("tag", "-a", name, "-m", message, rev, date=date)

    def tags(self) -> list[str]:
        return self.git("tag", "--list").split()

    def files(self, rev: str = "HEAD") -> list[str]:
        """Tracked file paths at ``rev``, sorted."""
        return sorted(self.git("ls-tree", "-r", "--name-only", rev).split("\n")[:-1])

    @property
    def url(self) -> str:
        """``file://`` URL a clone can use."""
        return self.path.resolve().as_uri()


def skill_md(name: str, description: str = "A fixture skill.") -> str:
    return f"---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nBody of {name}.\n"


def rule_md(name: str, body: str = "Follow the fixture rule.") -> str:
    return f"---\ndescription: Rule {name}.\n---\n\n{body}\n"


def agent_md(name: str, description: str = "A fixture agent.") -> str:
    return f"---\nname: {name}\ndescription: {description}\n---\n\nYou are {name}.\n"


def monorepo(
    path: Path,
    *,
    skills: tuple[str, ...] = ("alpha", "beta", "gamma"),
    rules: tuple[str, ...] = ("style", "naming"),
    agents: tuple[str, ...] = ("reviewer",),
    license_text: str | None = MIT_TEXT,
) -> GitRepo:
    """Build a one-revision monorepo: ``skills/<n>/SKILL.md``, ``rules/<n>.md``, ``agents/<n>.md``."""
    repo = GitRepo.init(path)
    files: dict[str, str | bytes] = {}
    for n in skills:
        files[f"skills/{n}/SKILL.md"] = skill_md(n)
    for n in rules:
        files[f"rules/{n}.md"] = rule_md(n)
    for n in agents:
        files[f"agents/{n}.md"] = agent_md(n)
    if license_text is not None:
        files["LICENSE"] = license_text
    repo.record("initial monorepo", files, date=BASE_DATE)
    return repo


def marketplace(
    path: Path,
    external: GitRepo,
    *,
    external_subdir: str = "plugins/ext",
    name: str = "fixture-market",
) -> GitRepo:
    """Build a Claude plugin marketplace repository.

    Holds a relative plugin ``local`` (``./plugins/local`` with one skill) and
    a ``git-subdir`` plugin ``ext`` pointing at ``external`` (``file://``) and
    ``external_subdir``. The caller records the plugin content in ``external``.
    """
    repo = GitRepo.init(path)
    document = {
        "name": name,
        "owner": {"name": "Fixture Owner"},
        "plugins": [
            {"name": "local", "source": "./plugins/local", "description": "A local plugin."},
            {
                "name": "ext",
                "source": {"source": "git-subdir", "url": external.url, "path": external_subdir},
                "description": "An external plugin.",
            },
        ],
    }
    repo.record(
        "marketplace",
        {
            ".claude-plugin/marketplace.json": json.dumps(document, indent=2) + "\n",
            "plugins/local/skills/local-skill/SKILL.md": skill_md("local-skill"),
            "LICENSE": MIT_TEXT,
        },
        date=BASE_DATE,
    )
    return repo
