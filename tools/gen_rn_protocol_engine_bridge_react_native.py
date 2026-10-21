#!/usr/bin/env python3
"""Generate ProtocolEngineHostBridgeReactNative.kt and thin @ReactMethod bodies in OfflineProtocolModule."""

from __future__ import annotations

import pathlib
import re
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
GEN_BRIDGE = REPO / "tools/gen_protocol_engine_host_bridge.py"
MODULE = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt"
OUT = REPO / "bindings/react-native/android/src/main/java/com/offlineprotocol/ProtocolEngineHostBridgeReactNative.kt"

METHODS = re.findall(
    r'"(\w+)"',
    GEN_BRIDGE.read_text().split("METHODS = [", 1)[1].split("]", 1)[0],
)


def parse_react_method(module: str, name: str) -> tuple[str, str, str]:
    """Return (full params without promise, promise param name, reject code snippet hint)."""
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
    return ", ".join(other), promise_part, find_error_code(module, name)


def find_error_code(module: str, name: str) -> tuple[str, str]:
    """Best-effort ERROR_* code and message prefix from existing body."""
    pat = rf"fun {re.escape(name)}\([^)]*\)\s*\{{"
    m = re.search(pat, module)
    if not m:
        return "ERROR_BRIDGE", f"Failed to run {name}"
    start = m.end()
    depth = 1
    i = start
    while i < len(module) and depth:
        if module[i] == "{":
            depth += 1
        elif module[i] == "}":
            depth -= 1
        i += 1
    body = module[start : i - 1]
    code_m = re.search(r'promise\.reject\("([A-Z0-9_]+)"', body)
    msg_m = re.search(r'promise\.reject\("[^"]+",\s*"([^"]+)"', body)
    code = code_m.group(1) if code_m else "ERROR_BRIDGE"
    msg = msg_m.group(1) if msg_m else f"Failed to run {name}"
    if msg.endswith(":"):
        msg = msg[:-1]
    if ": ${" in msg:
        msg = msg.split(": ${", 1)[0]
    return code, msg


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


def generate_react_native_kt(module: str) -> str:
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
        "    private fun run(",
        "        module: OfflineProtocolModule,",
        "        promise: Promise,",
        "        errorCode: String,",
        "        errorMessage: String,",
        "        block: (ProtocolEngineHost, Context) -> Any?,",
        "    ) {",
        "        try {",
        "            val host = module.engineHost",
        "            val context = module.reactApplicationContext",
        "            val result = block(host, context)",
        "            promise.resolve(BridgeResults.toReactNativeJsonStringOrPassThrough(result))",
        "        } catch (e: Exception) {",
        "            module.rejectBridgePromise(promise, e, errorCode, errorMessage)",
        "        }",
        "    }",
        "",
    ]
    for name in METHODS:
        params, _, (code, msg) = parse_react_method(module, name)
        if params.strip():
            sig = f"module: OfflineProtocolModule, {params}, promise: Promise"
            arg_names = [p.split(":")[0].strip() for p in params.split(",") if p.strip()]
            call_args = ", ".join(["module"] + arg_names + ["promise"])
        else:
            sig = "module: OfflineProtocolModule, promise: Promise"
            call_args = "module, promise"
        args_expr = build_args_expr(params)
        if args_expr == "null":
            invoke = delegate_call(name, params)
            body = f"""
    fun {name}({sig}) {{
        run(module, promise, "{code}", "{msg}") {{ host, context ->
            {invoke}
        }}
    }}
"""
        elif args_expr == "configJson":
            invoke = f'ProtocolEngineHostBridge.{name}(host, context, configJson)'
            body = f"""
    fun {name}({sig}) {{
        run(module, promise, "{code}", "{msg}") {{ host, context ->
            {invoke}
        }}
    }}
"""
        else:
            invoke = delegate_call(name, params)
            body = f"""
    fun {name}({sig}) {{
        run(module, promise, "{code}", "{msg}") {{ host, context ->
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
        params, _, _ = parse_react_method(module, name)
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
    if "fun rejectBridgePromise" not in module:
        insert = textwrap.dedent(
            """
            internal fun rejectBridgePromise(
                promise: Promise,
                error: Throwable,
                fallbackCode: String,
                fallbackMessage: String,
            ) {
                rejectWithProtocolError(promise, error, fallbackCode, fallbackMessage)
            }

            """
        )
        marker = "     * fallback code and \"$fallbackMessage: <cause>\".\n     */\n    private fun rejectWithProtocolError("
        module = module.replace(
            marker,
            "     * fallback code and \"$fallbackMessage: <cause>\".\n     */\n" + insert + "    private fun rejectWithProtocolError(",
            1,
        )
    return module


def main() -> None:
    module = MODULE.read_text()
    OUT.write_text(generate_react_native_kt(module))
    module = ensure_module_helpers(module)
    module = replace_method_bodies(module)
    MODULE.write_text(module)
    print(f"wrote {OUT.relative_to(REPO)} and updated {MODULE.relative_to(REPO)} ({len(METHODS)} methods)")


if __name__ == "__main__":
    main()
