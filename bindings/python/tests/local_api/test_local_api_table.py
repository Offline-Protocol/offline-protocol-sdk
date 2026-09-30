"""The method table: fresh against the definition, classified in full, and
in agreement with the chapter.

The Rust guard in the FFI crate holds the same three claims from the other
side; this is the copy that runs with the Python suite alone.
"""

from __future__ import annotations

import importlib.util
import re
from pathlib import Path

import pytest

from offline_protocol_sdk.local_api import (
    BEFORE_HELLO,
    EXPOSED,
    ID_RESULTS,
    METHOD_GROUPS,
    PLATFORM,
    SESSION_METHODS,
)
from offline_protocol_sdk.local_api import codec, dispatch
from offline_protocol_sdk.local_api.table import TABLE, UDL_SHA256
from offline_protocol_sdk.offline_protocol import ProtocolError

PACKAGE = Path(__file__).resolve().parents[2]
REPO = PACKAGE.parents[1]
GENERATOR = PACKAGE / "scripts" / "generate_local_api_table.py"
CHAPTER = REPO / "docs" / "spec" / "local-api.md"


def _generator():
    spec = importlib.util.spec_from_file_location("generate_local_api_table", GENERATOR)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


@pytest.mark.skipif(not GENERATOR.exists(), reason="the generator ships with the repository only")
def test_table_is_fresh_against_the_definition():
    generator = _generator()
    if not generator.UDL.exists():
        pytest.skip("the interface definition ships with the repository only")
    rendered = generator.generate()
    current = generator.OUTPUT.read_text(encoding="utf-8")
    assert rendered == current, "table.py is stale: run bindings/python/scripts/generate_local_api_table.py"
    assert UDL_SHA256 in rendered


def test_every_declaration_is_classified_exactly_once():
    names = dispatch.all_wire_names()
    assert EXPOSED | PLATFORM == names, {
        "unclassified": sorted(names - EXPOSED - PLATFORM),
        "not in the definition": sorted((EXPOSED | PLATFORM) - names),
    }
    assert not (EXPOSED & PLATFORM)
    assert len(EXPOSED) + len(PLATFORM) == len(names)


def test_session_methods_are_not_declarations():
    assert not (SESSION_METHODS & dispatch.all_wire_names())
    assert BEFORE_HELLO <= EXPOSED
    assert ID_RESULTS <= EXPOSED
    for group in METHOD_GROUPS.values():
        assert group <= EXPOSED


def test_run_loop_and_drain_are_never_on_the_wire():
    for name in ("process", "receive_message", "start", "stop", "pause", "resume", "constructor"):
        assert name in PLATFORM
        assert name not in EXPOSED


def test_error_codes_are_zero_based_positions_in_the_definition():
    variants = TABLE["errors"]["ProtocolError"]
    assert variants[0] == "NotStarted"
    assert codec.taxonomy_error("NotStarted", "x").code == -32000
    assert codec.taxonomy_error("PermissionDenied", "x").code == -32000 - variants.index("PermissionDenied")
    assert codec.taxonomy_error(variants[-1], "x").code == -32000 - (len(variants) - 1)
    # Every generated error class maps, and by name, not by the binding's
    # own (one-based) discriminant.
    for position, variant in enumerate(variants):
        exc = getattr(ProtocolError, variant)("detail")
        error = codec.error_from_protocol(exc)
        assert error.code == -32000 - position
        assert error.data == {"variant": variant}
        assert error.message == "detail"


def _chapter_sets() -> tuple[set[str], set[str], set[str]]:
    text = CHAPTER.read_text(encoding="utf-8")

    def section(start: str, end: str) -> str:
        return text.split(start, 1)[1].split(end, 1)[0]

    exposed = {
        line.split("`")[1]
        for line in section("## Method table", "## Platform operations").splitlines()
        if line.startswith("| `")
    }
    platform: set[str] = set()
    for line in section("## Platform operations", "## Event catalogue").splitlines():
        if line.startswith("| `"):
            platform.update(re.findall(r"`([^`]+)`", line.split("|")[1]))
    tags = {
        line.split("`")[1]
        for line in section("### The catalogue", "### Shapes").splitlines()
        if line.startswith("| `")
    }
    return exposed, platform, tags


@pytest.mark.skipif(not CHAPTER.exists(), reason="the chapter ships with the repository only")
def test_dispatch_agrees_with_the_chapter():
    exposed, platform, tags = _chapter_sets()
    assert exposed == EXPOSED, {"chapter only": sorted(exposed - EXPOSED), "server only": sorted(EXPOSED - exposed)}
    assert platform == PLATFORM, {"chapter only": sorted(platform - PLATFORM), "server only": sorted(PLATFORM - platform)}
    assert len(tags) >= 60
