#!/usr/bin/env python3
"""Generate ProtocolEngineHostBridgeReactNative.kt and thin @ReactMethod bodies in OfflineProtocolModule."""

from __future__ import annotations

import json
import pathlib
import re
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
GEN_BRIDGE = REPO / "tools/gen_protocol_engine_host_bridge.py"
MODULE = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt"
OUT = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/ProtocolEngineHostBridgeReactNative.kt"
MANIFEST = REPO / "tools/rn_engine_bridge_error_manifest.json"

METHODS = re.findall(
    r'"(\w+)"',
    GEN_BRIDGE.read_text().split("METHODS = [", 1)[1].split("]", 1)[0],
)


def parse_react_method(module: str, name: str) -> tuple[str, str]:
    """Return (full params without promise, promise param name)."""
    pat = rf"@ReactMethod\s*\n\s*fun {re.escape(name)}\(([^)]*)\)\s*\{{"
    m = re.search(pat, module)
    if not m:
        raise SystemExit(f"missing @ReactMethod {name}")
    params = m.group(1)
    parts = [p.strip() for p in params.split(",") if p.strip()]
    promise_part = parts[-1]
    if "Promise" not in promise_part:
        raise SystemExit(f"{name}: last param must be Promise")
    other = parts[:-1]
    return ", ".join(other), promise_part


def load_manifest() -> dict[str, dict[str, str]]:
    return json.loads(MANIFEST.read_text())


def error_for(name: str, manifest: dict[str, dict[str, str]]) -> tuple[str, str]:
    entry = manifest[name]
    return entry["code"], entry["message"]


def flags_for(name: str, manifest: dict) -> str:
    """The `run` arguments after the code and message, in a fixed order.

    `mapProtocolErrors` is true for the methods that rejected through
    `rejectWithProtocolError` before the shared bridge, so they keep the typed
    codes (`NoKeyPackage`, `SessionNotReady`) apps branch on; every other method
    keeps the fixed code it always rejected with. `parseJsonResult` is true for a
    method whose shared bridge returns a JSON string where React Native has
    always resolved an object. Any other string reaches JavaScript unchanged,
    because `src/index.ts` parses the ones it expects as strings and
    `JSON.parse` on an object throws.
    """
    entry = manifest[name]
    flags = ""
    if entry.get("mapProtocolErrors"):
        flags += ", mapProtocolErrors = true"
    if entry.get("parseJsonResult"):
        flags += ", parseJsonResult = true"
    return flags


def build_args_expr(params: str) -> str:
    if not params.strip():
        return "null"
    pieces = []
    for part in params.split(","):
        part = part.strip()
        if not part:
            continue
        name = part.split(":")[0].strip()
        typ = part.split(":")[1].strip() if ":" in part else ""
        if "ReadableMap" in typ:
            pieces.append(f'"{name}" to BridgeResults.readableMapToArgumentMap({name})')
        elif "ReadableArray" in typ:
            pieces.append(f'"{name}" to BridgeResults.readableArrayToIntList({name})')
        else:
            pieces.append(f'"{name}" to {name}')
    if len(pieces) == 1 and pieces[0].startswith('"configJson"'):
        return "configJson"
    return "mapOf(" + ", ".join(pieces) + ")"


def delegate_call(name: str, params: str) -> str:
    args = build_args_expr(params)
    if args == "null":
        arg_pass = "null"
    elif args == "configJson":
        arg_pass = "configJson"
    else:
        arg_pass = args
    return f"ProtocolEngineHostBridge.{name}(host, context, {arg_pass})"


def generate_react_native_kt(module: str, manifest: dict[str, dict[str, str]]) -> str:
    lines = [
        "package com.offlineprotocol",
        "",
        "import android.content.Context",
        "import com.facebook.react.bridge.Promise",
        "import com.facebook.react.bridge.ReactApplicationContext",
        "import com.facebook.react.bridge.ReadableArray",
        "import com.facebook.react.bridge.ReadableMap",
        "",
        "/**",
        " * React Native adapters for [ProtocolEngineHostBridge]. Every method listed in",
        " * `tools/gen_protocol_engine_host_bridge.py` delegates here so Flutter and RN",
        " * share one implementation.",
        " */",
        "internal object ProtocolEngineHostBridgeReactNative {",
        "",
        "    /**",
        "     * Runs one shared-bridge method for a React Native promise.",
        "     *",
        "     * A string result is resolved as the string: `src/index.ts` parses the",
        "     * ones it expects as JSON, and `JSON.parse` on an object throws.",
        "     * [parseJsonResult] is set only where React Native has always resolved",
        "     * an object and the shared bridge returns its JSON. [mapProtocolErrors]",
        "     * is set only for the methods that always surfaced typed protocol",
        "     * codes. Both come from `tools/rn_engine_bridge_error_manifest.json`.",
        "     */",
        "    private fun run(",
        "        module: OfflineProtocolModule,",
        "        promise: Promise,",
        "        errorCode: String,",
        "        errorMessage: String,",
        "        mapProtocolErrors: Boolean = false,",
        "        parseJsonResult: Boolean = false,",
        "        block: (ProtocolEngineHost, Context) -> Any?,",
        "    ) {",
        "        try {",
        "            val host = module.engineHost",
        "            val context = module.bridgeContext",
        "            val result = block(host, context)",
        "            promise.resolve(",
        "                if (parseJsonResult) BridgeResults.parseJsonResult(result) else BridgeResults.toReactNative(result),",
        "            )",
        "        } catch (e: Exception) {",
        "            module.rejectBridgePromise(promise, e, errorCode, errorMessage, mapProtocolErrors)",
        "        }",
        "    }",
        "",
    ]
    for name in METHODS:
        params, _ = parse_react_method(module, name)
        code, msg = error_for(name, manifest)
        if params.strip():
            sig = f"module: OfflineProtocolModule, {params}, promise: Promise"
            arg_names = [p.split(":")[0].strip() for p in params.split(",") if p.strip()]
            call_args = ", ".join(["module"] + arg_names + ["promise"])
        else:
            sig = "module: OfflineProtocolModule, promise: Promise"
            call_args = "module, promise"
        args_expr = build_args_expr(params)
        if args_expr == "configJson":
            invoke = f"ProtocolEngineHostBridge.{name}(host, context, configJson)"
        else:
            invoke = delegate_call(name, params)
        flags = flags_for(name, manifest)
        body = f"""
    fun {name}({sig}) {{
        run(module, promise, "{code}", "{msg}"{flags}) {{ host, context ->
            {invoke}
        }}
    }}
"""
        lines.append(textwrap.dedent(body).strip())
        lines.append("")
    lines.append("}")
    return "\n".join(lines) + "\n"


def replace_method_bodies(module: str) -> str:
    for name in METHODS:
        params, _ = parse_react_method(module, name)
        if params.strip():
            arg_names = [p.split(":")[0].strip() for p in params.split(",") if p.strip()]
            call = f"ProtocolEngineHostBridgeReactNative.{name}(this, {', '.join(arg_names)}, promise)"
        else:
            call = f"ProtocolEngineHostBridgeReactNative.{name}(this, promise)"
        new_body = f"        {call}"
        module = replace_fun_body(module, name, new_body)
    return module


def replace_fun_body(module: str, name: str, new_body: str) -> str:
    pat = rf"(@ReactMethod\s*\n\s*fun {re.escape(name)}\([^)]*\)\s*)\{{"
    m = re.search(pat, module)
    if not m:
        raise SystemExit(f"cannot replace body for {name}")
    start = m.end()
    depth = 1
    i = start
    while i < len(module) and depth:
        if module[i] == "{":
            depth += 1
        elif module[i] == "}":
            depth -= 1
        i += 1
    prefix = module[: m.end()]
    suffix = module[i - 1 :]
    return prefix + f"\n{new_body}\n    " + suffix


def ensure_module_helpers(module: str) -> str:
    if "internal val engineHost" not in module:
        module = module.replace("private val engineHost", "internal val engineHost", 1)
    if "internal val bridgeContext" not in module:
        insert = textwrap.dedent(
            """
                /** React context for shared bridge adapters (same module, not subclass-accessible). */
                internal val bridgeContext: Context
                    get() = reactApplicationContext

            """
        )
        module = module.replace(
            "    internal val engineHost by lazy {",
            insert + "    internal val engineHost by lazy {",
            1,
        )
        if "import android.content.Context" not in module:
            module = module.replace(
                "import android.app.Application",
                "import android.app.Application\nimport android.content.Context",
                1,
            )
    if "fun rejectBridgePromise" not in module:
        insert = textwrap.dedent(
            """
            internal fun rejectBridgePromise(
                promise: Promise,
                error: Throwable,
                fallbackCode: String,
                fallbackMessage: String,
                mapProtocolErrors: Boolean,
            ) {
                if (mapProtocolErrors) {
                    rejectWithProtocolError(promise, error, fallbackCode, fallbackMessage)
                } else {
                    promise.reject(fallbackCode, "$fallbackMessage: ${error.message}", error)
                }
            }

            """
        )
        module = module.replace(
            "    private fun rejectWithProtocolError(",
            insert + "    private fun rejectWithProtocolError(",
            1,
        )
    return module


def verify_generated_errors(generated: str, manifest: dict[str, dict[str, str]]) -> list[str]:
    problems: list[str] = []
    for name, entry in manifest.items():
        needle = f'run(module, promise, "{entry["code"]}", "{entry["message"]}"{flags_for(name, manifest)}) {{'
        if f"fun {name}(" in generated and needle not in generated:
            problems.append(f"{name}: expected `{needle}` in generated adapter")
    return problems


def main() -> None:
    import sys

    manifest = load_manifest()
    missing = [n for n in METHODS if n not in manifest]
    if missing:
        raise SystemExit(f"{MANIFEST.name} missing {missing}; run extract_rn_bridge_error_manifest.py")

    module = MODULE.read_text()
    generated = generate_react_native_kt(module, manifest)
    if "--check" in sys.argv:
        if OUT.read_text() != generated:
            print(f"{OUT.relative_to(REPO)} is stale; run {pathlib.Path(__file__).name}", file=sys.stderr)
            raise SystemExit(1)
        for name in METHODS:
            needle = f"ProtocolEngineHostBridgeReactNative.{name}("
            if needle not in module:
                print(
                    f"{MODULE.relative_to(REPO)} must delegate `{name}` through ProtocolEngineHostBridgeReactNative",
                    file=sys.stderr,
                )
                raise SystemExit(1)
        for problem in verify_generated_errors(generated, manifest):
            print(problem, file=sys.stderr)
            raise SystemExit(1)
        print(f"{OUT.name}: OK ({len(METHODS)} methods)")
        return
    OUT.write_text(generated)
    module = ensure_module_helpers(module)
    module = replace_method_bodies(module)
    MODULE.write_text(module)
    print(f"wrote {OUT.relative_to(REPO)} and updated {MODULE.relative_to(REPO)} ({len(METHODS)} methods)")


if __name__ == "__main__":
    main()
