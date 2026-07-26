# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Shared ``.agents/skills`` pool refcount + Kiro global-scope inertness.

Wave-1 vendor expansion introduces two guards that need end-to-end proof:

1. **Refcount guard** (``prune::reap_dropped_clients``): Codex, Gemini, Zed,
   and Amp all target the same ``.agents/skills/<name>`` directory. Removing
   one client's record must NOT delete a directory another client's output
   still references — the shared dir survives until the LAST referencing
   client drops it (adr_vendor_wave_expansion.md §3).
2. **Kiro global scoped rule** is written correctly at global scope but is
   inert until upstream Kiro #9176 closes; grim emits a render-layer warning
   citing the issue (self-heals on the upstream fix, no grim change).
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Shared-pool refcount: a dropped client's reap keeps the shared dir alive
# ---------------------------------------------------------------------------


def test_shared_agents_skills_survives_dropped_client_then_reaps_on_last(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Install one skill for codex+zed+amp → a single ``.agents/skills/<name>``
    dir recorded once per client. Narrowing ``[options].clients`` to drop zed
    and running ``update`` reaps zed's record but LEAVES the shared dir (codex
    and amp still reference it). A full uninstall finally removes the dir."""
    sk = make_artifact(
        f"{unique_repo}/shared-skill",
        "skill",
        {"shared-skill/SKILL.md": "---\nname: shared-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    shared_dir = bare_project_dir / ".agents/skills/shared-skill"

    # All three pool members select the same .agents/skills target.
    (bare_project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["codex", "zed", "amp"]\n\n'
        f'[skills]\nshared-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    # One physical directory, referenced once per client (3 outputs, 1 path).
    assert (shared_dir / "SKILL.md").is_file(), "the shared skill dir must exist after install"
    status = runner.json("status")["items"]
    item = next(r for r in status if r["name"] == "shared-skill")
    clients = {o["client"] for o in item["outputs"]}
    assert clients == {"codex", "zed", "amp"}, f"all three pool clients must record an output: {clients}"
    paths = {o["path"] for o in item["outputs"]}
    assert len(paths) == 1, f"all pool outputs must point at ONE shared dir: {paths}"

    # Drop zed from the client set; update reaps zed but the dir survives
    # because codex + amp still reference it.
    (bare_project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["codex", "amp"]\n\n'
        f'[skills]\nshared-skill = "{sk.fq}"\n'
    )
    update_rows = runner.json("update")["items"]
    row = next(r for r in update_rows if r["name"] == "shared-skill")
    assert "zed" in row.get("reaped_clients", []), (
        f"zed must be reported reaped when dropped from the client set: {row}"
    )
    assert (shared_dir / "SKILL.md").is_file(), (
        "the shared dir MUST survive while codex+amp still reference it (refcount guard)"
    )
    status_after = runner.json("status")["items"]
    item_after = next(r for r in status_after if r["name"] == "shared-skill")
    clients_after = {o["client"] for o in item_after["outputs"]}
    assert clients_after == {"codex", "amp"}, f"zed's output must be gone from status: {clients_after}"

    # Full uninstall removes the last references and the shared dir with them.
    runner.json("uninstall", "skill", "shared-skill")
    assert not shared_dir.exists(), "uninstalling the last references must remove the shared dir"


# ---------------------------------------------------------------------------
# Kiro global scoped rule: written correctly, warned as upstream-inert
# ---------------------------------------------------------------------------


def test_kiro_global_scoped_rule_writes_file_and_warns_upstream_inertness(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A scoped rule installed with ``--client kiro --global`` writes correct
    ``fileMatch`` steering to ``$HOME/.kiro/steering/<name>.md`` AND emits a
    render-layer warning citing upstream Kiro #9176 (global fileMatch is inert
    until that bug is fixed). The warning cites the issue number as a stable
    anchor, not exact prose."""
    ru = make_artifact(
        f"{unique_repo}/kiro-scoped",
        "rule",
        {"kiro-scoped.md": "---\npaths: ['**/*.rs']\n---\n# Kiro scoped\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\nkiro-scoped = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    result = runner.run(
        "install", "--global", "--client", "kiro", format="json", log_level="warn"
    )
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "installed", rows

    # Correct output IS written at global scope (self-heals when #9176 closes).
    steering = runner.home / ".kiro/steering/kiro-scoped.md"
    assert steering.is_file(), "global Kiro scoped rule must still be written at $HOME/.kiro/steering/"
    assert "fileMatch" in steering.read_text(), "the global steering file must carry fileMatch scoping"

    # Honest render-layer warning cites the upstream issue as a stable anchor.
    assert "9176" in result.stderr, (
        f"global Kiro scoped rule must warn citing upstream #9176; got: {result.stderr!r}"
    )


# ---------------------------------------------------------------------------
# `[options.vendors.<name>].shared_skills` — the rendering opt-in
# ---------------------------------------------------------------------------


def _skill(unique_repo: str, name: str, metadata: str = ""):
    """A skill artifact. ``metadata`` injects frontmatter ``metadata:`` keys —
    pass a tool-namespaced one to force both renderers off the verbatim fast
    path, which is what makes a byte-identity assertion mean anything."""
    body = f"---\nname: {name}\ndescription: d\n{metadata}---\n# body\n"
    return make_artifact(
        f"{unique_repo}/{name}",
        "skill",
        {f"{name}/SKILL.md": body},
        tag="v1",
    )


def test_shared_skills_flips_a_cursor_skill_into_the_pool_and_back(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Both directions of the opt-in, end to end. Cursor installs natively at
    ``.cursor/skills/<name>``; setting ``shared_skills = true`` moves it to the
    shared ``.agents/skills/<name>`` pool and reaps the native copy; clearing
    the key moves it back and reaps the pooled copy. ``status`` stays
    ``installed`` throughout and ``uninstall`` round-trips."""
    sk = _skill(unique_repo, "flip-skill")
    native = bare_project_dir / ".cursor/skills/flip-skill"
    pool = bare_project_dir / ".agents/skills/flip-skill"
    config = bare_project_dir / "grimoire.toml"
    config.write_text(
        '[options]\nclients = ["cursor"]\n\n'
        f'[skills]\nflip-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")

    assert (native / "SKILL.md").is_file(), "the resting layout is Cursor's own skills dir"
    assert not pool.exists(), "nothing may reach the pool before the opt-in"

    # ── off → on ──────────────────────────────────────────────────────────
    runner.json("config", "set", "options.vendors.cursor.shared_skills", "true")
    rows = runner.json("install")["items"]
    assert all(r["status"] in ("installed", "unchanged", "updated") for r in rows), rows

    assert (pool / "SKILL.md").is_file(), "the opt-in must render into the shared pool"
    assert not native.exists(), "the old native copy must be reaped, not left as a duplicate"
    item = next(r for r in runner.json("status")["items"] if r["name"] == "flip-skill")
    assert item["state"] == "installed", item
    [output] = item["outputs"]
    assert output["client"] == "cursor"
    assert output["path"].replace("\\", "/").endswith(".agents/skills/flip-skill"), output
    assert not output.get("modified"), "a fresh migration must not report drift"

    # ── on → off ──────────────────────────────────────────────────────────
    runner.json("config", "unset", "options.vendors.cursor.shared_skills")
    runner.json("install")

    assert (native / "SKILL.md").is_file(), "clearing the key must move the skill home"
    assert not pool.exists(), "the pooled copy must be reaped on the reverse flip"
    item = next(r for r in runner.json("status")["items"] if r["name"] == "flip-skill")
    assert item["state"] == "installed", item
    assert item["outputs"][0]["path"].replace("\\", "/").endswith(".cursor/skills/flip-skill")

    runner.json("uninstall", "skill", "flip-skill")
    assert not native.exists() and not pool.exists()


def test_shared_skills_global_flip_reanchors_the_record(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """The global-scope half, where the recorded ANCHOR moves rather than just
    the relative path: ``cursor-root`` → ``agents-skills``.

    This is the case the pooled candidate anchor exists for. Without it
    ``from_target`` returns ``UnknownAnchor``, and at an unchanged pin
    ``output_at_current_layout`` reads that as "already current" — so the flip
    silently does nothing at all. Bump the pin too and the same error is
    propagated from the materialize loop, after the files are on disk."""
    sk = _skill(unique_repo, "glob-flip")
    (grim_home / "grimoire.toml").write_text(f'[skills]\nglob-flip = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "cursor")

    native = runner.home / ".cursor/skills/glob-flip"
    pool = runner.home / ".agents/skills/glob-flip"
    state_path = grim_home / "state/global.json"
    assert (native / "SKILL.md").is_file()

    def anchors() -> list[str]:
        state = json.loads(state_path.read_text())
        [record] = [r for r in state["records"] if r["name"] == "glob-flip"]
        return [o["target"]["anchor"] for o in record["outputs"]]

    assert anchors() == ["cursor-root"], anchors()

    runner.json("config", "set", "--global", "options.vendors.cursor.shared_skills", "true")
    rows = runner.json("install", "--global", "--client", "cursor")["items"]
    assert all(r["status"] in ("installed", "unchanged", "updated") for r in rows), rows

    assert (pool / "SKILL.md").is_file(), "global skills must land in $HOME/.agents/skills"
    assert not native.exists(), "the stranded native copy must be reaped"
    assert anchors() == ["agents-skills"], f"the record must re-anchor, got {anchors()}"

    item = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "glob-flip")
    assert item["state"] == "installed", item

    runner.json("uninstall", "--global", "skill", "glob-flip")
    assert not pool.exists()


def test_shared_skills_flip_preserves_a_hand_edited_copy_and_warns(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """ADR sub-decision A7-a. A copy the user edited is never deleted by the
    layout-migration reaper, so the flip leaves both files on disk — and
    because skill scanning is additive, the client would silently see the same
    skill twice. The warning is therefore mandatory and must name both
    absolute paths and the client.

    Driven through ``grim update``, not plain ``install``: a hand-edited
    recorded output trips the integrity gate first and returns ``Refused``."""
    sk = _skill(unique_repo, "edited-skill")
    native_doc = bare_project_dir / ".cursor/skills/edited-skill/SKILL.md"
    pool_doc = bare_project_dir / ".agents/skills/edited-skill/SKILL.md"
    (bare_project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["cursor"]\n\n'
        f'[skills]\nedited-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")
    assert native_doc.is_file()

    native_doc.write_text("---\nname: edited-skill\ndescription: d\n---\n# MINE\n")
    runner.json("config", "set", "options.vendors.cursor.shared_skills", "true")

    result = runner.run("update", format="json", log_level="warn")
    assert pool_doc.is_file(), "the new pooled copy must still be written"
    assert native_doc.read_text().endswith("# MINE\n"), (
        "a hand-edited copy at the old path must be preserved verbatim — never deleted"
    )
    stderr = result.stderr
    assert "edited-skill" in stderr and "preserved" in stderr, (
        f"the kept-modified warning is mandatory; got: {stderr!r}"
    )
    for path in (".cursor/skills/edited-skill", ".agents/skills/edited-skill"):
        assert path in stderr.replace("\\", "/"), (
            f"the warning must name both absolute paths ({path} missing); got: {stderr!r}"
        )
    assert "cursor" in stderr, f"the warning must name the client that now sees both; got: {stderr!r}"


def test_an_opted_in_client_joins_the_pool_refcount_without_clobbering_it(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An opted-in client becomes a first-class pool member: it shares the ONE
    physical directory with the vendors that pool natively, records its own
    output against it, and the refcount guard keeps the directory alive when
    one of them is dropped. Codex's recorded hash must survive Cursor joining
    — the byte-identical universal render is what makes that true, and it is
    why a vendor declaring ``skill_fields`` may never be pool-capable."""
    # A tool-namespaced metadata key forces BOTH renderers off the verbatim
    # fast path, so the byte-identity this test relies on is actually
    # exercised rather than trivially true for an untransformed document.
    sk = _skill(unique_repo, "join-skill", metadata="metadata:\n  claude.user-invocable: true\n")
    pool = bare_project_dir / ".agents/skills/join-skill"
    config = bare_project_dir / "grimoire.toml"
    config.write_text(
        '[options]\nclients = ["codex"]\n\n'
        f'[skills]\njoin-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")
    assert (pool / "SKILL.md").is_file()

    # Cursor joins the same directory via the opt-in.
    config.write_text(
        '[options]\nclients = ["codex", "cursor"]\n\n'
        "[options.vendors.cursor]\nshared_skills = true\n\n"
        f'[skills]\njoin-skill = "{sk.fq}"\n'
    )
    runner.json("install")

    item = next(r for r in runner.json("status")["items"] if r["name"] == "join-skill")
    assert {o["client"] for o in item["outputs"]} == {"codex", "cursor"}, item
    assert len({o["path"] for o in item["outputs"]}) == 1, (
        f"both clients must record the ONE shared dir: {item['outputs']}"
    )
    assert not any(o.get("modified") for o in item["outputs"]), (
        "a joining pool member must not invalidate the incumbent's content hash"
    )
    assert not (bare_project_dir / ".cursor/skills").exists(), (
        "an opted-in client must not also write its native skills dir"
    )

    # Dropping codex leaves the dir alive — cursor still references it.
    config.write_text(
        '[options]\nclients = ["cursor"]\n\n'
        "[options.vendors.cursor]\nshared_skills = true\n\n"
        f'[skills]\njoin-skill = "{sk.fq}"\n'
    )
    row = next(r for r in runner.json("update")["items"] if r["name"] == "join-skill")
    assert "codex" in row.get("reaped_clients", []), row
    assert (pool / "SKILL.md").is_file(), (
        "the shared dir must survive while the opted-in client still references it"
    )

    runner.json("uninstall", "skill", "join-skill")
    assert not pool.exists(), "the last reference dropping must remove the shared dir"


def test_a_flip_refuses_to_clobber_a_hand_authored_file_at_the_new_path(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The untracked-clobber gate must survive a layout move.

    Regression: the gate keyed on *client* — "does this client have a record
    anywhere?" — while ``shared_skills`` lets the user move that client's
    destination. A client with a record at its native path therefore counted as
    tracked at the pool path it had never written, so the flip silently
    ``remove_path``-ed a hand-authored ``.agents/skills/<name>`` with no
    refusal, no warning, and no ``--force``. The byte-identical file with no
    record at all was correctly refused, which is the tell.

    The gate now compares the recorded ``(anchor, relative)`` pair against the
    one the current layout produces, so a moved destination is untracked until
    grim has actually written it."""
    sk = _skill(unique_repo, "clobber-skill")
    pool_doc = bare_project_dir / ".agents/skills/clobber-skill/SKILL.md"
    native_doc = bare_project_dir / ".cursor/skills/clobber-skill/SKILL.md"
    (bare_project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["cursor"]\n\n'
        f'[skills]\nclobber-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")
    assert native_doc.is_file()

    # A hand-authored skill of the same name already sitting in the pool —
    # exactly what a user who curates `.agents/skills` by hand would have.
    pool_doc.parent.mkdir(parents=True)
    pool_doc.write_text("---\nname: clobber-skill\ndescription: hand written\n---\n# NOT GRIM'S\n")

    runner.json("config", "set", "options.vendors.cursor.shared_skills", "true")
    result = runner.run("install", format="json", check=False)
    assert result.returncode == 65, (
        f"flipping onto an untracked destination must be refused, got "
        f"{result.returncode}; {result.stderr}"
    )
    assert pool_doc.read_text().endswith("# NOT GRIM'S\n"), (
        "the hand-authored file must survive the refusal untouched"
    )
    assert native_doc.is_file(), "the refusal must not have moved anything yet"

    # --force is the documented remedy, and it completes the migration.
    runner.json("install", "--force")
    assert "NOT GRIM'S" not in pool_doc.read_text(), "--force must overwrite as it always has"
    assert not native_doc.parent.exists(), "the forced flip still reaps the old native copy"


def test_a_flip_does_not_refuse_a_pool_dir_a_sibling_client_already_records(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The other half of the untracked-clobber gate: a destination grim wrote
    for a SIBLING client, in this very record, is not untracked.

    The shared pool is one physical directory that every pool member records
    an output against, so keying "untracked" on the client alone would refuse
    grim's own directory the moment a flip and a content change land together
    — the only combination where the footprints differ and the gate actually
    fires."""
    repo = f"{unique_repo}/sibling-skill"
    v1 = make_artifact(
        repo,
        "skill",
        {"sibling-skill/SKILL.md": "---\nname: sibling-skill\ndescription: d\n---\n# v1\n"},
        tag="v1",
    )
    pool = bare_project_dir / ".agents/skills/sibling-skill"
    config = bare_project_dir / "grimoire.toml"
    config.write_text(
        '[options]\nclients = ["codex", "cursor"]\n\n'
        f'[skills]\nsibling-skill = "{v1.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")
    assert (pool / "SKILL.md").read_text().endswith("# v1\n")
    assert (bare_project_dir / ".cursor/skills/sibling-skill/SKILL.md").is_file()

    # New content AND the flip in one step: cursor's destination becomes the
    # pool dir codex owns, and the bytes there no longer match what this
    # install would write.
    v2 = make_artifact(
        repo,
        "skill",
        {"sibling-skill/SKILL.md": "---\nname: sibling-skill\ndescription: d\n---\n# v2\n"},
        tag="v2",
    )
    config.write_text(
        '[options]\nclients = ["codex", "cursor"]\n\n'
        "[options.vendors.cursor]\nshared_skills = true\n\n"
        f'[skills]\nsibling-skill = "{v2.fq}"\n'
    )
    runner.run("lock", check=False)
    result = runner.run("install", format="json", check=False)
    assert result.returncode == 0, (
        "grim's own pool directory, recorded by a sibling client, must not be "
        f"treated as untracked; got {result.returncode}: {result.stderr}"
    )
    assert (pool / "SKILL.md").read_text().endswith("# v2\n")
    # Asserted against the record, not `status`: the first install created
    # `.cursor/`, which makes detection non-empty, and `status` reconciles a
    # record against *detected* clients only — codex has no project marker
    # here and would be filtered out for reasons unrelated to this test.
    state = json.loads((bare_project_dir / ".grimoire/state.json").read_text())
    [record] = [r for r in state["records"] if r["name"] == "sibling-skill"]
    assert {o["client"] for o in record["outputs"]} == {"codex", "cursor"}, record
    assert len({o["target"]["relative"] for o in record["outputs"]}) == 1, (
        f"both clients must record the ONE shared dir: {record['outputs']}"
    )
    assert not (bare_project_dir / ".cursor/skills/sibling-skill").exists(), (
        "cursor's old native copy must still be reaped"
    )


def test_a_flip_onto_a_live_symlink_refuses_instead_of_erroring(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A symlink where the flip lands must reach the forceable refusal, not a
    hard I/O error.

    ``footprint_hash`` stats with ``symlink_metadata``, so a symlink pointing
    at a *directory* is not ``is_dir()`` and gets hashed as a file — then the
    read follows it into the directory and fails ``EISDIR``, aborting the whole
    install with exit 74. A skill destination is a directory, so that is the
    ordinary shape here. The gate previously refused only a *dangling* link."""
    sk = _skill(unique_repo, "symlink-skill")
    native = bare_project_dir / ".cursor/skills/symlink-skill"
    pool = bare_project_dir / ".agents/skills"
    (bare_project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["cursor"]\n\n'
        f'[skills]\nsymlink-skill = "{sk.fq}"\n'
    )
    runner = grim_at(bare_project_dir)
    runner.run("lock", check=False)
    runner.json("install")
    assert (native / "SKILL.md").is_file()

    pool.mkdir(parents=True)
    (pool / "symlink-skill").symlink_to(native, target_is_directory=True)

    runner.json("config", "set", "options.vendors.cursor.shared_skills", "true")
    result = runner.run("install", format="json", check=False)
    assert result.returncode == 65, (
        f"a live symlink at the destination must be a forceable refusal, not an "
        f"I/O error; got {result.returncode}: {result.stderr}"
    )
    assert (pool / "symlink-skill").is_symlink(), "the refusal must not touch the link"

    # --force unlinks the link itself and writes a real directory; the link
    # target is never followed into.
    runner.json("install", "--force")
    assert not (pool / "symlink-skill").is_symlink(), "--force must replace the link"
    assert (pool / "symlink-skill/SKILL.md").is_file()
