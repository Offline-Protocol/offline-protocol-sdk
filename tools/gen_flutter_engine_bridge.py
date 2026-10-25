#!/usr/bin/env python3
"""Generate FlutterEngineChannel.kt dispatch arms from method_channel_methods.txt."""

from __future__ import annotations

import pathlib
import sys
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
METHODS_FILE = REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
OUT_KT = (
    REPO
    / "bindings/flutter/offline_protocol/android/src/main/kotlin/com/offlineprotocol/flutter/FlutterEngineChannel.kt"
)

# Methods handled outside the generic dispatch (plugin lifecycle hooks).
PLUGIN_ONLY = frozenset()


def load_methods() -> list[str]:
    lines = METHODS_FILE.read_text().splitlines()
    return [ln.strip() for ln in lines if ln.strip() and not ln.strip().startswith("#")]


def render() -> str:
    methods = load_methods()
    arms: list[str] = []
    for m in methods:
        if m in PLUGIN_ONLY:
            continue
        arms.append(f'        "{m}" -> EngineMethodHandlers.{m}(host, env, arguments)')

    body = textwrap.dedent(
        f"""
        package com.offlineprotocol.flutter

        import android.content.Context
        import com.offlineprotocol.ProtocolEngineHost

        /**
         * Method-channel dispatch for the Flutter plugin (F3). Every name in
         * `method_channel_methods.txt` must have a matching arm here.
         */
        object FlutterEngineChannel {{
            data class Environment(
                val context: Context,
                val activityProvider: () -> android.app.Activity?,
                val internetHandlers: ProtocolEngineHost.InternetTransportEventHandlers,
                val replayEvents: (fromSeq: Int, toSeq: Int) -> List<Map<String, Any>>,
                val tearDownProtocol: () -> Unit,
                val createOnEngine: (configJson: String) -> Unit,
                val startOnEngine: () -> Unit,
                val stopOnEngine: () -> Unit,
                val seedAppState: () -> uniffi.offline_protocol.AppState,
                val discardOneShotEvent: (key: String) -> Unit,
            )

            @Throws(Throwable::class)
            fun dispatch(
                host: ProtocolEngineHost,
                env: Environment,
                method: String,
                arguments: Any?,
            ): Any? = when (method) {{
        {chr(10).join(arms)}
                else -> throw IllegalArgumentException("Unknown method: $method")
            }}
        }}
        """
    ).strip() + "\n"

    return body


def main() -> int:
    text = render()
    methods = load_methods()
    if "--check" in sys.argv:
        if not OUT_KT.exists() or OUT_KT.read_text() != text:
            print(f"{OUT_KT} is stale; run tools/gen_flutter_engine_bridge.py", file=sys.stderr)
            return 1
        return 0
    OUT_KT.parent.mkdir(parents=True, exist_ok=True)
    OUT_KT.write_text(text)
    print(f"Wrote {OUT_KT} ({len(methods)} methods)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
