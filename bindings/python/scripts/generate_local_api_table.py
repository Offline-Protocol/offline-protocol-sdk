#!/usr/bin/env python3
"""Generates the local API's method table from the interface definition.

Reads ``crates/offline-protocol-uniffi/src/offline_protocol.udl`` and writes
``offline_protocol_sdk/local_api/table.py``: every declaration of the three
objects and the namespace with its parameters and result type, every enum
with its variants in declaration order, every dictionary with its fields, and
the error enum in order. The reference server dispatches from that table and
classifies each declaration as exposed or platform-only in ``dispatch.py``.

The table is checked in rather than generated at import so that a change to
the definition shows up as a diff, and so that a declaration nobody has
classified fails a test rather than being exposed or hidden by default. A
test regenerates the table in memory and compares; a Rust guard in the FFI
crate reads the definition, the chapter and the classification and asserts
the three agree.

Usage::

    python bindings/python/scripts/generate_local_api_table.py [--check]

``--check`` exits non-zero when the checked-in table is stale.
"""

from __future__ import annotations

import argparse
import hashlib
import pprint
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKAGE = HERE.parent / "offline_protocol_sdk"
REPO = HERE.parents[2]
UDL = REPO / "crates" / "offline-protocol-uniffi" / "src" / "offline_protocol.udl"
OUTPUT = PACKAGE / "local_api" / "table.py"

#: The wire prefix of each object's methods. The chapter keeps each object's
#: own prefix so the guard can map every row back to its declaration.
PREFIXES = {
    "OfflineProtocol": "",
    "MeshServices": "services.",
    "DataStore": "data.",
    "namespace": "",
}

_BLOCK = re.compile(
    r"(?:\[(?P<attrs>[^\]]*)\]\s*)?"
    r"(?P<kind>callback interface|namespace|interface|dictionary|enum)\s+"
    r"(?P<name>\w+)\s*\{(?P<body>.*?)\};",
    re.DOTALL,
)
_METHOD = re.compile(
    r"^(?:\[(?P<attrs>[^\]]*)\]\s*)?(?P<ret>[\w<>?, ]+?)\s+(?P<name>\w+)\s*\((?P<params>.*)\)$",
    re.DOTALL,
)
_CTOR = re.compile(
    r"^(?:\[(?P<attrs>[^\]]*)\]\s*)?constructor\s*\((?P<params>.*)\)$",
    re.DOTALL,
)
_FIELD = re.compile(r"^(?P<type>[\w<>?, ]+?)\s+(?P<name>\w+)\s*(?:=\s*(?P<default>.+))?$")


def _strip_comments(text: str) -> str:
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def _norm_type(text: str) -> str:
    return re.sub(r"\s+", "", text)


def _split_params(text: str) -> list[str]:
    """Splits a parameter list on the commas at angle-bracket depth zero."""
    out: list[str] = []
    depth = 0
    current = ""
    for ch in text:
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(current)
            current = ""
        else:
            current += ch
    if current.strip():
        out.append(current)
    return [p.strip() for p in out if p.strip()]


def _parse_params(text: str) -> tuple[tuple[str, str], ...]:
    params = []
    for item in _split_params(text):
        head, _, name = item.rpartition(" ")
        if not head:
            raise ValueError(f"cannot parse parameter {item!r}")
        params.append((name.strip(), _norm_type(head)))
    return tuple(params)


def _parse_methods(body: str) -> dict[str, tuple[tuple[tuple[str, str], ...], str]]:
    methods: dict[str, tuple[tuple[tuple[str, str], ...], str]] = {}
    for statement in body.split(";"):
        statement = statement.strip()
        if not statement:
            continue
        ctor = _CTOR.match(statement)
        if ctor:
            attrs = ctor.group("attrs") or ""
            named = re.search(r"Name\s*=\s*(\w+)", attrs)
            name = named.group(1) if named else "constructor"
            methods[name] = (_parse_params(ctor.group("params")), "object")
            continue
        method = _METHOD.match(statement)
        if not method:
            raise ValueError(f"cannot parse declaration {statement!r}")
        methods[method.group("name")] = (
            _parse_params(method.group("params")),
            _norm_type(method.group("ret")),
        )
    return methods


def _parse_fields(body: str) -> tuple[tuple[str, str, bool], ...]:
    fields = []
    for statement in body.split(";"):
        statement = statement.strip()
        if not statement:
            continue
        field = _FIELD.match(statement)
        if not field:
            raise ValueError(f"cannot parse field {statement!r}")
        fields.append(
            (field.group("name"), _norm_type(field.group("type")), field.group("default") is not None)
        )
    return tuple(fields)


def parse_udl(text: str) -> dict:
    """The table as a plain dict, from the definition's text."""
    clean = _strip_comments(text)
    enums: dict[str, tuple[str, ...]] = {}
    records: dict[str, tuple[tuple[str, str, bool], ...]] = {}
    callbacks: list[str] = []
    methods: dict[str, dict] = {}
    errors: dict[str, tuple[str, ...]] = {}
    for block in _BLOCK.finditer(clean):
        kind, name, body = block.group("kind"), block.group("name"), block.group("body")
        attrs = block.group("attrs") or ""
        if kind == "enum":
            variants = tuple(re.findall(r'"(\w+)"', body))
            if "Error" in attrs:
                errors[name] = variants
            else:
                enums[name] = variants
        elif kind == "dictionary":
            records[name] = _parse_fields(body)
        elif kind == "callback interface":
            callbacks.append(name)
        elif kind in ("interface", "namespace"):
            key = "namespace" if kind == "namespace" else name
            methods[key] = _parse_methods(body)
    for expected in ("OfflineProtocol", "MeshServices", "DataStore", "namespace"):
        if expected not in methods:
            raise ValueError(f"the definition has no {expected} block")
    return {
        "enums": enums,
        "errors": errors,
        "records": records,
        "callbacks": tuple(callbacks),
        "methods": methods,
    }


def wire_names(table: dict) -> dict[str, tuple[str, str]]:
    """Every declaration by its wire name: ``name -> (object, declaration)``."""
    names: dict[str, tuple[str, str]] = {}
    for obj, prefix in PREFIXES.items():
        for declaration in table["methods"][obj]:
            names[prefix + declaration] = (obj, declaration)
    return names


def render(table: dict, udl_text: str) -> str:
    digest = hashlib.sha256(udl_text.encode("utf-8")).hexdigest()
    body = pprint.pformat(table, width=100, sort_dicts=True)
    return (
        '"""The interface definition as data, generated. Do not edit.\n'
        "\n"
        "Written by ``bindings/python/scripts/generate_local_api_table.py`` from\n"
        "``crates/offline-protocol-uniffi/src/offline_protocol.udl``. A test\n"
        "regenerates it in memory and fails when this file is stale, so every\n"
        "change to the definition reaches the reference server as a diff here\n"
        "and a classification in ``dispatch.py``.\n"
        '"""\n'
        "\n"
        "from __future__ import annotations\n"
        "\n"
        f'UDL_SHA256 = "{digest}"\n'
        "\n"
        f"TABLE = {body}\n"
    )


def generate() -> str:
    text = UDL.read_text(encoding="utf-8")
    return render(parse_udl(text), text)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail when the table is stale")
    args = parser.parse_args(argv)
    rendered = generate()
    if args.check:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current != rendered:
            print(f"{OUTPUT} is stale; run {Path(__file__).name}", file=sys.stderr)
            return 1
        return 0
    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"wrote {OUTPUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
