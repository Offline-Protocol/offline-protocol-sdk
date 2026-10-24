#!/usr/bin/env python3
"""Generate FlutterEngineChannel.kt dispatch arms from method_channel_methods.txt."""

from __future__ import annotations

import pathlib
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


def main() -> None:
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

    OUT_KT.parent.mkdir(parents=True, exist_ok=True)
    OUT_KT.write_text(body)
    print(f"Wrote {OUT_KT} ({len(methods)} methods)")


if __name__ == "__main__":
    main()
