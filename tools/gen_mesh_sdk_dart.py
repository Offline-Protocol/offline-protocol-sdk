#!/usr/bin/env python3
"""Generate mesh_sdk.dart MethodChannel wrappers from method_channel_methods.txt."""

from __future__ import annotations

import pathlib
import textwrap

REPO = pathlib.Path(__file__).resolve().parents[1]
METHODS = [
    ln.strip()
    for ln in (
        REPO / "bindings/flutter/offline_protocol/method_channel_methods.txt"
    ).read_text().splitlines()
    if ln.strip()
]
OUT = REPO / "bindings/flutter/offline_protocol/lib/src/mesh_sdk.dart"

# Methods already on OfflineProtocol class (lifecycle / events).
SKIP = {
    "create",
    "destroy",
    "start",
    "stop",
    "initializeMlsWithSecureStorage",
    "localAddress",
    "sendMessage",
    "enableTransport",
    "replayEvents",
}

HEADER = textwrap.dedent(
    """
    import 'dart:convert';

    import 'package:flutter/services.dart';

    import 'offline_protocol.dart';

    /// Typed wrappers for every native method-channel entry point (React Native parity).
    class MeshSdk {
      MeshSdk._();

      static const MethodChannel _channel =
          MethodChannel('com.offlineprotocol/offline_protocol');

      static Future<T?> _invoke<T>(String method, [Object? arguments]) {
        return _channel.invokeMethod<T>(method, arguments);
      }
    """
)

FOOTER = "}\n"


def method_fn(name: str) -> str:
    if name in SKIP:
        return ""
    # Generic map-based invoke; apps pass the same keys as React Native where applicable.
    return textwrap.dedent(
        f"""
      /// Native `{name}` (see React Native `OfflineProtocolNativeModule.{name}`).
      static Future<dynamic> {name}([Map<String, dynamic>? args]) =>
          _invoke<dynamic>('{name}', args);
    """
    )


def main() -> None:
    parts = [HEADER]
    for m in METHODS:
        if m not in SKIP:
            parts.append(method_fn(m))
    parts.append(FOOTER)
    OUT.write_text("".join(parts))
    print(f"Wrote {OUT}")


if __name__ == "__main__":
    main()
