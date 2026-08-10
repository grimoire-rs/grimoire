# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim context` acceptance tests — read-only resolution introspection."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from src.helpers import write_config

# `grim context` is read-only introspection and never dials a registry, so
# the browse-filter tests below take a literal host instead of the session
# `registry` fixture. Depending on that fixture would put them in its blast
# radius for no benefit: it is session-scoped and skips the test when no
# container answers. Same reasoning as
# `test_context_registry_authenticated_from_docker_config` above, which has
# always used a literal.
_OFFLINE_REGISTRY = "registry.example"


def _project(project_dir: Path, registry: str) -> None:
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "test"\noci = "{registry}"\ndefault = true\n\n'
        "[skills]\n\n[rules]\n"
    )


def test_context_reports_scope_paths_clients_registries(
    grim_at, project_dir: Path, registry: str
) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)

    doc = runner.json("context")
    assert doc["scope"] == "project"
    assert doc["workspace"] == str(project_dir)
    assert doc["config_path"] == str(project_dir / "grimoire.toml")
    assert doc["config_exists"] is True
    assert doc["lock_path"] == str(project_dir / "grimoire.lock")
    assert doc["lock_exists"] is False
    assert "lock_error" in doc and doc["lock_error"] is None
    assert doc["state_path"] == str(project_dir / ".grimoire" / "state.json")
    assert doc["grim_home"], "grim_home always resolves"
    assert doc["version"], "version always present"
    assert doc["offline"] is False
    assert "offline_source" in doc and doc["offline_source"] is None
    # .claude/ marker present ⇒ claude detected as an effective client.
    assert "claude" in doc["clients"], doc["clients"]
    regs = doc["registries"]
    assert any(
        r["alias"] == "test" and r["url"] == registry and r["default"] and r["kind"] == "registry"
        for r in regs
    ), regs
    # `authenticated` is always present as a bool; the isolated HOME has no
    # docker config, so nothing is authenticated.
    for r in regs:
        assert isinstance(r["authenticated"], bool)
        assert r["authenticated"] is False, r
    assert doc["default_registry"] == registry


def test_context_reports_an_unreadable_lock(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`lock_exists` answers "is it there", `lock_error` answers "can grim use it".

    `context` is the command a user reaches for when install state looks
    wrong; reporting a bare `exists` for a lock no other command can read
    points the diagnosis away from the actual fault.
    """
    _project(project_dir, registry)
    (project_dir / "grimoire.lock").write_text("not a lock at all\n[[[\n")
    runner = grim_at(project_dir)

    doc = runner.json("context")
    assert doc["lock_exists"] is True
    assert doc["lock_error"], "an unreadable lock must be reported, not just 'exists'"

    plain = runner.plain("context")
    assert any("unreadable" in ln for ln in plain.stdout.splitlines()), plain.stdout
    # Read-only introspection: reporting the fault is not failing on it.
    assert plain.returncode == 0


def test_context_registry_authenticated_from_docker_config(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """A stored credential for the registry's host flips `authenticated`.

    Offline and hermetic: `grim context` never dials the registry, and the
    credential presence is derived from a temp `$DOCKER_CONFIG` file — no
    real keychain or network involved.
    """
    _project(project_dir, "private.example.com/team")
    docker_dir = tmp_path / "docker"
    docker_dir.mkdir()
    # An `auths` entry for the host (namespace path stripped by the probe).
    (docker_dir / "config.json").write_text(
        '{"auths": {"private.example.com": {"auth": "dXNlcjpwYXNz"}}}'
    )

    runner = grim_at(project_dir)
    runner.env["DOCKER_CONFIG"] = str(docker_dir)
    doc = runner.json("context")

    reg = next(r for r in doc["registries"] if r["url"] == "private.example.com/team")
    assert reg["authenticated"] is True, doc["registries"]


def test_context_global_scope(grim_at, project_dir: Path, grim_home: Path) -> None:
    runner = grim_at(project_dir)
    doc = runner.json("context", "--global")
    assert doc["scope"] == "global"
    assert doc["workspace"] == str(grim_home)
    assert doc["config_path"] == str(grim_home / "grimoire.toml")


def test_context_offline_flag_flips_source(grim_at, project_dir: Path, registry: str) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)
    doc = runner.json("context", "--offline")
    assert doc["offline"] is True
    assert doc["offline_source"] == "flag"


def test_context_outside_project_exits_79(grim_at, tmp_path: Path) -> None:
    outside = tmp_path / "empty"
    outside.mkdir()
    runner = grim_at(outside)
    result = runner.plain("context", check=False)
    assert result.returncode == 79, result.stderr


def test_context_plain_is_key_value_table(grim_at, project_dir: Path, registry: str) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)
    result = runner.plain("context")
    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[0].startswith("Key"), lines[0]
    assert any("scope" in ln and "project" in ln for ln in lines), result.stdout


# ── Browse filters (plan C-020 / S-019) ────────────────────────────────


def _filtered_project(project_dir: Path, registry: str = _OFFLINE_REGISTRY) -> None:
    """Two sources: one carrying a browse filter, one deliberately without."""
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "filtered"\noci = "{registry}"\ndefault = true\n'
        'include = ["acme/platform/**", "acme/tools/**"]\n'
        'exclude = ["acme/platform/legacy/**"]\n\n'
        f'[[registries]]\nalias = "plain"\noci = "{registry}/other"\n\n'
        "[skills]\n\n[rules]\n"
    )


def test_context_reports_the_authored_browse_filters(grim_at, project_dir: Path) -> None:
    """`registries[]` carries `include`/`exclude` verbatim, `[]` when unset.

    `grim context` is how a JSON-interface consumer learns what a browse
    would show without parsing `grimoire.toml` itself, so the patterns must
    come back in declaration order and both keys must be present on **every**
    entry — an absent key cannot be told apart from an older grim
    (`subsystem-cli-api.md` bans `skip_serializing_if` here).
    """
    _filtered_project(project_dir)
    runner = grim_at(project_dir)

    regs = {r["alias"]: r for r in runner.json("context")["registries"]}

    assert regs["filtered"]["include"] == ["acme/platform/**", "acme/tools/**"], regs
    assert regs["filtered"]["exclude"] == ["acme/platform/legacy/**"], regs
    # Always-present, never omitted: an unfiltered source reports two empty
    # lists rather than dropping the keys.
    assert regs["plain"]["include"] == [], regs
    assert regs["plain"]["exclude"] == [], regs


def test_context_plain_appends_filter_counts_only_when_filtered(
    grim_at, project_dir: Path
) -> None:
    """The plain row gains `, N include, M exclude`; an unfiltered row does not.

    Counts, not patterns — the registry cell is already the widest in the
    table and a glob list has no width bound. Omitting both clauses entirely
    keeps an unfiltered row byte-identical to what shipped before filters
    existed (plan C-020).
    """
    _filtered_project(project_dir)
    runner = grim_at(project_dir)

    result = runner.plain("context")
    assert result.returncode == 0, result.stderr
    rows = [ln for ln in result.stdout.splitlines() if ln.startswith("registry")]

    filtered = next(ln for ln in rows if "filtered" in ln)
    assert "2 include, 1 exclude" in filtered, filtered
    plain = next(ln for ln in rows if ln.split()[1] == "plain")
    assert "include" not in plain and "exclude" not in plain, plain


def test_context_registry_flag_reports_no_filter(grim_at, project_dir: Path) -> None:
    """`--registry` reports empty lists even when the config declares patterns.

    The forced browse set is genuinely unfiltered (plan C-009), so reporting
    the config's patterns here would misdescribe what a `--registry` run
    does — the same asymmetry `grim search --registry` has (S-014).
    """
    _filtered_project(project_dir)
    runner = grim_at(project_dir)

    regs = runner.json("context", "--registry", _OFFLINE_REGISTRY)["registries"]
    assert len(regs) == 1, regs
    assert regs[0]["include"] == [], regs
    assert regs[0]["exclude"] == [], regs


def test_context_on_a_malformed_browse_filter_exits_78(grim_at, project_dir: Path) -> None:
    """`context` is the reference answer `grim search` was measured against (B1).

    The Block this pairs with was a `grim search` that swallowed an
    uncompilable browse pattern and browsed a different registry set at exit
    0, while `context` on the identical file exited 78. The fix aligned
    `search` onto *this* behaviour, so the claim "both commands agree" needs
    a test on both sides — the sibling lives in `test_registries.py` as
    `test_malformed_project_filter_never_browses_the_fallback_set_b1`.

    The valid-pattern control comes second and is what keeps the exit-78
    assertion about the pattern: the same file, the same registry, one
    character changed, and `context` reports the source normally.
    """
    broken = (
        '[[registries]]\nalias = "acme"\noci = "ghcr.io/acme"\n'
        'include = ["acme/["]\ndefault = true\n'
    )
    (project_dir / "grimoire.toml").write_text(broken)
    runner = grim_at(project_dir)

    result = runner.plain("context", check=False)
    assert result.returncode == 78, (
        f"an uncompilable browse pattern must exit 78 (EX_CONFIG); "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert str(project_dir / "grimoire.toml") in result.stderr, (
        f"the diagnostic must name the file to fix; got: {result.stderr!r}"
    )
    assert "acme/[" in result.stderr, (
        f"the diagnostic must quote the offending pattern; got: {result.stderr!r}"
    )
    assert result.stdout.strip() == "", (
        f"a config grim refused to validate must report no context at all; "
        f"got: {result.stdout!r}"
    )

    (project_dir / "grimoire.toml").write_text(broken.replace('acme/[', 'acme/**'))
    fixed = runner.json("context")
    assert [r["include"] for r in fixed["registries"]] == [["acme/**"]], (
        f"control: the only difference is the pattern itself; got: {fixed['registries']}"
    )


# ── The accept/reject boundary of `validate_filter_pattern` (S-015) ──────
#
# `include`/`exclude` are the first `grimoire.toml` values compiled into a
# regex program, so which strings the validator lets through is a released
# contract, not an implementation detail. These two tables pin the edges:
# useless-but-legal globs stay exit 0 (tightening one into a rejection would
# break a config that parses today), and the three genuinely uncompilable
# shapes stay exit 78 with the offending pattern quoted. Neither table was
# covered anywhere before — the shipped tests only exercised well-formed
# patterns and the single `acme/[` case above.


def _one_include(project_dir: Path, pattern: str) -> None:
    """A minimal single-source project whose only filter is *pattern*.

    `json.dumps` writes the TOML array: a JSON string literal and a TOML
    basic string share their escape rules, so a pattern containing a
    backslash survives the round trip through the file unchanged.
    """
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "acme"\noci = "{_OFFLINE_REGISTRY}/acme"\n'
        f"default = true\ninclude = {json.dumps([pattern])}\n"
    )


@pytest.mark.parametrize(
    "pattern",
    [
        pytest.param("**", id="match-everything"),
        pytest.param("*", id="single-segment-star"),
        pytest.param("**/**", id="redundant-globstars"),
        pytest.param("{}", id="empty-alternation"),
        pytest.param("a{,}", id="empty-alternate-branch"),
        pytest.param("/", id="bare-separator"),
        pytest.param(".", id="cur-dir"),
        pytest.param("..", id="parent-dir"),
        pytest.param("./x", id="cur-dir-prefixed"),
        pytest.param("\\\\", id="escaped-backslash"),
    ],
)
def test_context_accepts_degenerate_but_valid_filter_patterns(
    grim_at, project_dir: Path, pattern: str
) -> None:
    """A pattern that compiles is accepted and stored verbatim, however useless.

    `..` and `./x` matter most here: they look like path traversal but a
    browse filter is matched against a repository-name string, never joined
    onto a filesystem path, so there is nothing to escape — and rejecting
    them would be a new failure for a config that loads today.
    `{}` / `a{,}` are the `empty_alternates(true)` dialect knob the ADR
    pins; they are legal *because* of that setting.
    """
    _one_include(project_dir, pattern)

    doc = grim_at(project_dir).json("context")
    assert doc["registries"][0]["include"] == [pattern], (
        f"a compilable pattern must round-trip verbatim; got: {doc['registries']}"
    )


@pytest.mark.parametrize(
    "pattern",
    [
        pytest.param("[]", id="unclosed-character-class"),
        pytest.param("{", id="unclosed-alternate-group"),
        pytest.param("\\", id="dangling-backslash"),
    ],
)
def test_context_rejects_uncompilable_filter_patterns(
    grim_at, project_dir: Path, pattern: str
) -> None:
    """The three shapes globset cannot compile are exit 78, pattern quoted."""
    _one_include(project_dir, pattern)

    result = grim_at(project_dir).plain("context", check=False)
    assert result.returncode == 78, (
        f"pattern {pattern!r} must exit 78 (EX_CONFIG); "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert f"'{pattern}'" in result.stderr, (
        f"the diagnostic must quote the offending pattern; got: {result.stderr!r}"
    )
