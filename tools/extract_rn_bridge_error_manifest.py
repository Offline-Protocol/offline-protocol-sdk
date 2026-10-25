#!/usr/bin/env python3
"""Refresh tools/rn_engine_bridge_error_manifest.json from RN module reject semantics.

Reads the committed React Native module sources in this repo. Methods that
delegate through ProtocolEngineHostBridgeReactNative keep their manifest entry
until you edit the manifest by hand or restore inline reject bodies temporarily.

After changing reject semantics for a shared engine method:

  python3 tools/extract_rn_bridge_error_manifest.py
  python3 tools/gen_rn_protocol_engine_bridge_react_native_swift.py
  python3 tools/gen_rn_protocol_engine_bridge_react_native.py
  bash scripts/check-binding-generators.sh --check
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
OUT = REPO / "tools/rn_engine_bridge_error_manifest.json"
IOS = REPO / "bindings/react-native/ios/OfflineProtocolModule.swift"
KT = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt"
GEN_BRIDGE = REPO / "tools/gen_protocol_engine_host_bridge.py"

METHODS = re.findall(
    r'"(\w+)"',
    GEN_BRIDGE.read_text().split("METHODS = [", 1)[1].split("]", 1)[0],
)

DEFAULT_CODE = "ERROR_BRIDGE"
DEFAULT_MSG = "Failed to run {name}"


def ios_method_body(module: str, name: str) -> str | None:
    pat = re.compile(
        rf"(?:@objc func {re.escape(name)}|@objc\({re.escape(name)}:[^)]*\)\s*\n\s*func \w+)\(",
        re.MULTILINE,
    )
    m = pat.search(module)
    if not m:
        return None
    open_brace = module.find("{", m.end() - 1)
    if open_brace < 0:
        return None
    start = open_brace + 1
    depth = 1
    i = start
    while i < len(module) and depth:
        if module[i] == "{":
            depth += 1
        elif module[i] == "}":
            depth -= 1
        i += 1
    return module[start : i - 1]


def kotlin_method_body(module: str, name: str) -> str | None:
    pat = rf"@ReactMethod\s*\n\s*fun {re.escape(name)}\("
    m = re.search(pat, module)
    if not m:
        return None
    open_brace = module.find("{", m.end() - 1)
    if open_brace < 0:
        return None
    start = open_brace + 1
    depth = 1
    i = start
    while i < len(module) and depth:
        if module[i] == "{":
            depth += 1
        elif module[i] == "}":
            depth -= 1
        i += 1
    return module[start : i - 1]


def normalize_message(msg: str) -> str:
    if msg.endswith(":"):
        msg = msg[:-1]
    for sep in (": ${", ": \\(error", ": \\("):
        if sep in msg:
            msg = msg.split(sep, 1)[0]
    return msg.strip()


def parse_ios_body(body: str, name: str) -> tuple[str, str, bool] | None:
    if "ProtocolEngineHostBridgeReactNative." in body:
        return None
    m = re.search(
        r"rejectWithProtocolError\([^,]+,\s*[^,]+,\s*fallbackCode:\s*\"([A-Z0-9_]+)\",\s*fallbackMessage:\s*\"([^\"]+)\"",
        body,
    )
    if m:
        return m.group(1), normalize_message(m.group(2)), True
    codes = re.findall(r'rejecter\(\s*"([A-Z0-9_]+)"', body)
    if codes:
        code = codes[0]
        if code == "NOT_STARTED" and len(codes) > 1:
            code = codes[1]
        mm = re.search(rf'rejecter\(\s*"{re.escape(code)}"\s*,\s*"([^"]+)"', body)
        msg = normalize_message(mm.group(1)) if mm else DEFAULT_MSG.format(name=name)
        return code, msg, False
    return None


def parse_kotlin_body(body: str, name: str) -> tuple[str, str, bool] | None:
    if "ProtocolEngineHostBridgeReactNative." in body:
        return None
    m = re.search(
        r"rejectWithProtocolError\([^,]+,\s*[^,]+,\s*\"([A-Z0-9_]+)\",\s*\"([^\"]+)\"",
        body,
    )
    if m:
        return m.group(1), normalize_message(m.group(2)), True
    m = re.search(r'promise\.reject\("([A-Z0-9_]+)"', body)
    if m:
        mm = re.search(r'promise\.reject\("[^"]+",\s*"([^"]+)"', body)
        msg = normalize_message(mm.group(1)) if mm else DEFAULT_MSG.format(name=name)
        return m.group(1), msg, False
    return None


def extract(module: str, body_fn, parse_fn) -> dict[str, dict]:
    """Code, message and whether the inline body mapped protocol errors.

    `mapProtocolErrors` records a body that rejected through
    `rejectWithProtocolError`: the generated adapter keeps the typed codes for
    exactly those methods (see `flags_for` in both adapter generators).
    """
    out: dict[str, dict] = {}
    for name in METHODS:
        body = body_fn(module, name)
        if not body:
            continue
        parsed = parse_fn(body, name)
        if parsed:
            code, msg, maps = parsed
            out[name] = {"code": code, "message": msg, "mapProtocolErrors": maps}
    return out


def main() -> int:
    base: dict[str, dict[str, str]] = {}
    if OUT.exists():
        base = json.loads(OUT.read_text())

    ios = IOS.read_text()
    kt = KT.read_text()
    scraped: dict[str, dict] = {}
    for found in (
        extract(kt, kotlin_method_body, parse_kotlin_body),
        extract(ios, ios_method_body, parse_ios_body),
    ):
        for name, entry in found.items():
            # A method that mapped on either platform keeps mapping on both.
            maps = entry["mapProtocolErrors"] or scraped.get(name, {}).get("mapProtocolErrors", False)
            scraped[name] = {**entry, "mapProtocolErrors": maps}

    # Merge rather than replace, so a hand-set field (`parseJsonResult`) survives
    # a re-run. A scraped body is authoritative for code, message and mapping.
    manifest: dict[str, dict] = {name: dict(entry) for name, entry in base.items()}
    for name, entry in scraped.items():
        merged = {**manifest.get(name, {}), "code": entry["code"], "message": entry["message"]}
        merged.pop("mapProtocolErrors", None)
        if entry["mapProtocolErrors"]:
            merged["mapProtocolErrors"] = True
        manifest[name] = merged

    for name in METHODS:
        if name not in manifest:
            manifest[name] = {"code": DEFAULT_CODE, "message": DEFAULT_MSG.format(name=name)}

    OUT.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    generic = [
        n
        for n in METHODS
        if manifest[n]["code"] == DEFAULT_CODE and manifest[n]["message"] == DEFAULT_MSG.format(name=n)
    ]
    print(
        f"wrote {OUT.relative_to(REPO)} ({len(METHODS)} methods, "
        f"{len(scraped)} scraped this run, {len(generic)} generic defaults)"
    )
    if generic and "--strict" in sys.argv:
        print("methods still on generic defaults:", generic, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
