# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The docs and AGENTS.md name the `GRIM_CONFIG` telemetry surface where readers look.

The stability page must list `GRIM_CONFIG` as Unstable, the configuration page
must carry its environment rows, and AGENTS.md must name consent as the gate
for hooks and settings.
"""

from __future__ import annotations

from pathlib import Path

from src.helpers import PROJECT_ROOT

DOCS = PROJECT_ROOT / "docs" / "src" / "content" / "docs"


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _table_rows(text: str, prefix: str) -> list[str]:
    return [line for line in text.splitlines() if line.startswith(prefix)]


def test_stability_page_lists_grim_config_as_unstable() -> None:
    text = _read(DOCS / "stability.md")
    start = text.index("**[Telemetry][telemetry].**")
    end = text.find("\n- **", start + 1)
    if end == -1:
        end = len(text)
    bullet = " ".join(text[start:end].split())
    for name in ("GRIM_CONFIG", "[telemetry]", "GRIM_TELEMETRY_OTLP_HEADERS"):
        assert name in bullet, f"Unstable telemetry bullet does not name {name}"


def test_configuration_env_table_names_grim_config() -> None:
    text = _read(DOCS / "configuration.md")
    for name in ("GRIM_CONFIG", "GRIM_TELEMETRY_OTLP_HEADERS"):
        assert _table_rows(text, f"| `{name}` "), f"env table has no row for {name}"
    assert "{#config-file}" in _read(DOCS / "telemetry.md")


def test_agents_md_names_consent_as_the_gate() -> None:
    rows = _table_rows(_read(PROJECT_ROOT / "AGENTS.md"), "| *(no variable)* |")
    assert len(rows) == 1
    row = rows[0].lower()
    assert "consent is the gate" in row
    assert "grim_allow_hooks" in row and "does not exist" in row
