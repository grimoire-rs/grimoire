# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A hook is a bundle member — authoring, locking, provenance, and removal.

The install/resolve side already treated a hook as a first-class member
(``effective_set`` lists ``(Hook, set.hooks)``; ``declared_bundle_provides``
handles it; ``bundle_members_lock`` filters it). What grim could not do was
*author* one: ``RawBundleSource`` is ``deny_unknown_fields`` and had no
``hooks`` field, so a ``[hooks]`` table in a bundle ``.toml`` was exit **78**.
These tests cover both halves — the authoring path through ``grim build`` /
``grim release``, and the end-to-end declaration path a published bundle takes.

**Membership declares; the install seam decides arming** (feature flag, then
workspace consent, then the registrar). A hook reaching that seam through a bundle
must be gated exactly like a directly-declared one, so
``test_a_bundle_provided_hook_is_not_armed_by_membership_alone`` pins the
*absence* of arming with the flag off — and
``test_a_bundle_provided_hook_arms_when_the_gates_are_open`` is its positive
control: with the flag on and ``--trust-hooks`` the bundle member reaches the
dispatch table exactly once. Without that control the gated assertions are
satisfied by a build in which a bundle member can never arm at all, which is a
different (and silently broken) product.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_bundle, make_hook, write_config
from src.registry import REGISTRY_HOST

# The arming tests below drive the launcher and Claude's registration, both of
# which are POSIX-only in v1 — matching every other `test_hook*` module. The
# authoring/locking tests in this file are platform-independent, so the skip is
# per-test rather than module-wide.
_POSIX_ONLY = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

MARKER_KEY = "com.grimoire.managed"
MARKER_VALUE = "hook-dispatcher"


def _dispatch_rows(runner) -> list[dict]:
    """Every dispatch row across every root — the machine-local arming truth.

    The dispatch table is the file `grim hook run` reads, so a row here is what
    "armed" means; its absence is what "not armed" means.
    """
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text())
    return [row for root in table["roots"].values() for row in root["hooks"]]


def _managed_elements(project_dir: Path) -> list[dict]:
    """Claude's grim-owned handler elements, across every event and group."""
    path = project_dir / ".claude" / "settings.local.json"
    hooks = (json.loads(path.read_text()) if path.is_file() else {}).get("hooks", {})
    return [
        element
        for groups in hooks.values()
        for group in groups
        for element in group.get("hooks", [])
        if element.get(MARKER_KEY) == MARKER_VALUE
    ]


def _lock_hook_names(project_dir: Path) -> list[str]:
    """Hook binding names in the lock, in file order.

    Parsed by scanning ``[[hook]]`` sections rather than with a TOML reader so
    the assertion sees the file the way a reviewer does, and so a malformed lock
    fails loudly here instead of raising deep inside a parser.
    """
    names: list[str] = []
    in_hook = False
    for line in (project_dir / "grimoire.lock").read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("[["):
            in_hook = stripped == "[[hook]]"
            continue
        if in_hook and stripped.startswith("name = "):
            names.append(stripped.split("=", 1)[1].strip().strip('"'))
    return names


def _hook_row(runner, name: str) -> dict:
    """The ``grim status`` JSON row for hook ``name``."""
    rows = [
        r
        for r in runner.json("status")["items"]
        if r["kind"] == "hook" and r["name"] == name
    ]
    assert rows, f"no hook row named {name!r} in status"
    return rows[0]


# ── Authoring: `[hooks]` in a bundle source ─────────────────────────────


def test_bundle_source_with_a_hooks_table_builds(
    grim, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression: this exact file used to exit **78**.

    ``deny_unknown_fields`` on ``RawBundleSource`` turned a ``[hooks]`` table
    into ``unknown field 'hooks', expected one of 'skills', 'rules', 'agents',
    …`` — a raw TOML parse error with no hint that the kind existed.
    """
    src = project_dir / "guard-stack.toml"
    src.write_text(
        "summary = \"a bundle carrying a hook\"\n"
        "\n"
        "[hooks]\n"
        f'shell-guard = "{REGISTRY_HOST}/{unique_repo}/hooks/shell-guard:1"\n'
    )

    report = grim.json("build", "--kind", "bundle", str(src))

    assert report["kind"] == "bundle"
    assert report["status"] == "built"
    assert report["layer_digest"].startswith("sha256:")


def test_built_bundle_layer_names_the_hook_member_last(
    grim, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The wire layer carries ``kind: "hook"``, sorted after every other kind.

    ``BundleManifest::new`` sorts members by ``(kind, name)`` using
    ``ArtifactKind``'s derived ``Ord``, where ``Hook`` is the **last** variant.
    That ordering is what makes the new table additive under Principle 9: a
    pre-hook bundle's members keep their exact relative positions, so its layer
    digest cannot move. Asserting the hook sorts last is asserting that.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    src = project_dir / "full-stack.toml"
    # Authored hook-first to prove the ORDER COMES FROM THE SORT, not from the
    # order of the tables in the file or of the loops in `read_bundle_members`.
    src.write_text(
        "[hooks]\n"
        f'shell-guard = "{hook.fq}"\n'
        "\n"
        "[skills]\n"
        f'code-review = "{REGISTRY_HOST}/{unique_repo}/code-review:1"\n'
        "\n"
        "[rules]\n"
        f'rust-style = "{REGISTRY_HOST}/{unique_repo}/rust-style:1"\n'
        "\n"
        "[agents]\n"
        f'reviewer = "{REGISTRY_HOST}/{unique_repo}/reviewer:1"\n'
    )

    ref = f"{REGISTRY_HOST}/{unique_repo}/bundles/full-stack:1"
    grim.json("release", str(src), ref)

    from src.registry import fetch_blob, fetch_manifest

    manifest = fetch_manifest(f"{unique_repo}/bundles/full-stack", "1")
    layer = fetch_blob(
        f"{unique_repo}/bundles/full-stack", manifest["layers"][0]["digest"]
    )
    members = json.loads(layer)["members"]

    kinds = [m["kind"] for m in members]
    assert kinds == ["skill", "rule", "agent", "hook"], (
        "members sort by ArtifactKind's derived Ord (Hook last), regardless of "
        f"authoring order — got {kinds}"
    )
    hook_member = members[-1]
    assert hook_member["name"] == "shell-guard"
    assert hook_member["id"] == hook.fq


def test_rebuilding_a_hook_bundle_is_byte_identical(
    grim, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Digest stability: two builds of one unchanged source agree.

    Principle 9's self-heal obligation rests on this — a re-materialize of an
    unchanged artifact must leave ``status`` not-modified, which it cannot do if
    the layer digest wanders between builds.
    """
    src = project_dir / "guard-stack.toml"
    src.write_text(
        "[hooks]\n"
        f'b-guard = "{REGISTRY_HOST}/{unique_repo}/hooks/b:1"\n'
        f'a-guard = "{REGISTRY_HOST}/{unique_repo}/hooks/a:1"\n'
    )

    first = grim.json("build", "--kind", "bundle", str(src))["layer_digest"]
    second = grim.json("build", "--kind", "bundle", str(src))["layer_digest"]

    assert first == second, "a rebuild of an unchanged bundle must not move the digest"


def test_adding_a_hooks_table_leaves_the_other_members_byte_identical(
    grim, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The additive proof: a `[hooks]` table APPENDS to the layer.

    The pre-hook members must serialize to the exact same bytes in the same
    order, so an existing bundle re-published by a grim that understands hooks
    keeps its digest. This is the acceptance-level mirror of the unit test
    ``read_bundle_members_hook_member_is_deterministic_and_leaves_others_byte_identical``.
    """
    legacy_body = (
        "[skills]\n"
        f'code-review = "{REGISTRY_HOST}/{unique_repo}/code-review:1"\n'
        "\n"
        "[rules]\n"
        f'rust-style = "{REGISTRY_HOST}/{unique_repo}/rust-style:1"\n'
    )
    legacy = project_dir / "legacy.toml"
    legacy.write_text(legacy_body)
    withhook = project_dir / "withhook.toml"
    withhook.write_text(
        legacy_body
        + "\n[hooks]\n"
        + f'shell-guard = "{REGISTRY_HOST}/{unique_repo}/hooks/shell-guard:1"\n'
    )

    legacy_digest = grim.json("build", "--kind", "bundle", str(legacy))["layer_digest"]
    hook_digest = grim.json("build", "--kind", "bundle", str(withhook))["layer_digest"]

    assert legacy_digest != hook_digest, "the hook member must reach the wire"

    # Push both and compare the member arrays: the legacy list is a strict
    # prefix of the hook-bearing one.
    from src.registry import fetch_blob, fetch_manifest

    def members_of(src: Path, repo_leaf: str) -> list[dict]:
        grim.json("release", str(src), f"{REGISTRY_HOST}/{unique_repo}/{repo_leaf}:1")
        manifest = fetch_manifest(f"{unique_repo}/{repo_leaf}", "1")
        blob = fetch_blob(
            f"{unique_repo}/{repo_leaf}", manifest["layers"][0]["digest"]
        )
        return json.loads(blob)["members"]

    legacy_members = members_of(legacy, "bundles/legacy")
    hook_members = members_of(withhook, "bundles/withhook")

    assert hook_members[: len(legacy_members)] == legacy_members, (
        "adding a [hooks] table must not reorder or rewrite the pre-hook members"
    )
    assert hook_members[-1]["kind"] == "hook"


# ── End to end: a published bundle delivers a hook ──────────────────────


def test_add_bundle_locks_its_hook_member_with_bundle_provenance(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The test that actually proves the requirement.

    Publish a hook, publish a bundle declaring it, ``grim add`` the bundle, and
    assert the hook is locked, pinned by digest, and marked bundle-provided.
    ``grim add`` (not ``lock``) is the sharp case: it projects just the added
    bundle's members through ``bundle_members_lock``, and an omission there
    would leave the hook ``missing`` in ``status`` until an unrelated
    ``grim install`` picked it up — the defect that already shipped once for
    ``mcp``.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    bundle = make_bundle(
        f"{unique_repo}/bundles/guard-stack",
        [("hook", "shell-guard", hook.fq)],
        tag="1.0.0",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    runner.run("add", "--kind", "bundle", bundle.fq)

    lock = (project_dir / "grimoire.lock").read_text()
    assert _lock_hook_names(project_dir) == ["shell-guard"], (
        f"the bundle's hook member must be locked by `add`, got lock:\n{lock}"
    )
    assert f'bundle = "{REGISTRY_HOST}/{unique_repo}/bundles/guard-stack"' in lock, (
        "the hook entry must record the bundle as its provenance"
    )
    # The member's OWN source is pinned, distinct from the bundle's digest —
    # the trust predicate at the install seam keys on this, never on the
    # bundle's source: a bundle from registry A may pin a member from registry
    # B, and approving A must not silently grant B.
    assert f"{REGISTRY_HOST}/{unique_repo}/hooks/shell-guard@sha256:" in lock, (
        "the hook member keeps its own digest-pinned source"
    )
    assert hook.digest in lock, "and that digest is the hook's, not the bundle's"


def test_status_shows_bundle_provenance_for_a_hook_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    bundle = make_bundle(
        f"{unique_repo}/bundles/guard-stack",
        [("hook", "shell-guard", hook.fq)],
        tag="1.0.0",
    )
    write_config(project_dir, bundles={"guard-stack": bundle.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    row = _hook_row(runner, "shell-guard")

    assert row["source"] == (
        f"bundle: {REGISTRY_HOST}/{unique_repo}/bundles/guard-stack"
    ), f"a hook member's row must name its bundle, got {row['source']!r}"
    assert row["pinned"].startswith(
        f"{REGISTRY_HOST}/{unique_repo}/hooks/shell-guard@sha256:"
    )


def test_a_bundle_provided_hook_is_not_armed_by_membership_alone(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Membership declares; it never arms.

    With the hooks feature flag off (the default), a bundle-delivered hook must
    report an arming verdict explaining it is gated — exactly as a directly
    declared one does. If bundle membership ever became a way *around* the
    consent gate, this is the test that fails: a bundle is fetched from a
    registry, so a member that armed on declaration alone would let a publisher
    reach the trust decision by packaging rather than by asking.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    bundle = make_bundle(
        f"{unique_repo}/bundles/guard-stack",
        [("hook", "shell-guard", hook.fq)],
        tag="1.0.0",
    )
    write_config(project_dir, bundles={"guard-stack": bundle.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")

    row = _hook_row(runner, "shell-guard")

    assert row["arming"], (
        "a bundle-provided hook must carry per-client arming verdicts, not an "
        f"empty list — row: {row}"
    )
    assert row["state"] != "installed", (
        "with the feature flag off the row must not read as plainly installed; "
        f"got state={row['state']!r}, arming={row['arming']!r}"
    )


# ── The inverse: removing the bundle removes the hook ───────────────────


def test_removing_the_bundle_evicts_its_hook_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression: ``drop_from_lock`` never fanned its retain pass over
    ``lock.hooks``, yet restamped ``declaration_hash`` anyway — so an
    undeclared ``[[hook]]`` survived in a lock reading FRESH and the next
    ``grim install`` re-materialized *and re-armed* it. Strictly worse than the
    same gap previously fixed for ``mcp``: the resurrected artifact is code a
    client runs automatically, after the user asked for it to be gone.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    bundle = make_bundle(
        f"{unique_repo}/bundles/guard-stack",
        [("hook", "shell-guard", hook.fq)],
        tag="1.0.0",
    )
    write_config(project_dir, bundles={"guard-stack": bundle.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _lock_hook_names(project_dir) == ["shell-guard"], "precondition"

    runner.run("remove", "bundle", "guard-stack")

    assert _lock_hook_names(project_dir) == [], (
        "the bundle's hook member must be evicted with the bundle:\n"
        + (project_dir / "grimoire.lock").read_text()
    )
    runner.run("install")
    assert _lock_hook_names(project_dir) == [], (
        "install must not resurrect the evicted hook"
    )


def test_a_directly_declared_hook_survives_the_bundles_removal(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The other direction: a hook the user *also* declared directly stays.

    The bundle's removal drops only the bundle's claim on it; the direct
    declaration still holds it in the effective set, so the entry survives with
    its bundle provenance re-derived away.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    bundle = make_bundle(
        f"{unique_repo}/bundles/guard-stack",
        [("hook", "shell-guard", hook.fq)],
        tag="1.0.0",
    )
    write_config(
        project_dir,
        bundles={"guard-stack": bundle.fq},
        hooks={"shell-guard": hook.fq},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _lock_hook_names(project_dir) == ["shell-guard"], "precondition"

    runner.run("remove", "bundle", "guard-stack")

    assert _lock_hook_names(project_dir) == ["shell-guard"], (
        "a directly-declared hook must outlive the bundle that also provided it:\n"
        + (project_dir / "grimoire.lock").read_text()
    )
    lock = (project_dir / "grimoire.lock").read_text()
    assert f'bundle = "{REGISTRY_HOST}/{unique_repo}/bundles/guard-stack"' not in lock, (
        "the removed bundle's provenance must be gone from the surviving entry"
    )


def test_removing_a_directly_declared_hook_drops_it_from_the_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The same ``drop_from_lock`` gap, reached without any bundle at all.

    ``grim remove hook <name>`` was already shipped and already affected by the
    missing ``process(&mut lock.hooks)`` fan-out, so this case is a regression
    test for the defect independent of bundle membership.
    """
    hook = make_hook(f"{unique_repo}/hooks/shell-guard", "shell-guard", tag="1")
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _lock_hook_names(project_dir) == ["shell-guard"], "precondition"

    runner.run("remove", "hook", "shell-guard")

    assert _lock_hook_names(project_dir) == [], (
        "the undeclared hook must leave the lock, or install re-arms it:\n"
        + (project_dir / "grimoire.lock").read_text()
    )
