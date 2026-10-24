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
    import 'package:flutter/services.dart';

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

TYPED_HELPERS = textwrap.dedent(
    """
      // --- Typed fast paths (avoid hand-built maps on hot calls) ---

      /// Disables telemetry and **awaits** the engine-thread flush (may take a few
      /// seconds). Use on lifecycle shutdown; do not call other engine methods until
      /// this future completes if you need a clean boundary (F8).
      static Future<void> disableTelemetryAndWait() => _invoke<void>('disableTelemetry');

      /// Ends the telemetry session (same blocking semantics as [disableTelemetryAndWait]).
      static Future<void> endTelemetrySessionAndWait() =>
          _invoke<void>('endTelemetrySession');

      /// Flushes buffered telemetry on the engine thread (same blocking semantics).
      static Future<void> flushTelemetryAndWait() => _invoke<void>('flushTelemetry');

      /// Plain send with explicit fields (same method-channel shape as `OfflineProtocol.sendMessage`).
      static Future<String> sendMessageTyped({
        required String recipient,
        required String content,
        int priority = 1,
        String? replyToMsg,
      }) async {
        final id = await _invoke<dynamic>('sendMessage', {
          'recipient': recipient,
          'content': content,
          'priority': priority,
          if (replyToMsg != null) 'replyToMsg': replyToMsg,
        });
        final messageId = id?.toString() ?? '';
        if (messageId.isEmpty) {
          throw StateError('sendMessage returned no message id');
        }
        return messageId;
      }

      /// Sends a rich message with explicit fields instead of a nested options map.
      static Future<dynamic> sendMessageRichTyped({
        required String recipient,
        required String content,
        required int priority,
        String? replyToMsg,
        Map<String, dynamic>? options,
      }) =>
          _invoke<dynamic>('sendMessageRich', {
            'recipient': recipient,
            'content': content,
            'priority': priority,
            if (replyToMsg != null) 'replyToMsg': replyToMsg,
            if (options != null) 'options': options,
          });

      /// Resolves protocol state without building an argument map.
      static Future<String> getStateTyped() async {
        final value = await _invoke<dynamic>('getState');
        return value?.toString() ?? 'Stopped';
      }

      /// Whether the Internet transport reports ready (false when not initialized).
      static Future<bool> internetIsReadyTyped() async {
        final value = await _invoke<dynamic>('internetIsReady');
        return value == true;
      }

      /// Whether the relay session was superseded elsewhere (false when not initialized).
      static Future<bool> internetIsSupersededTyped() async {
        final value = await _invoke<dynamic>('internetIsSuperseded');
        return value == true;
      }

      /// BLE radio state query used before showing a manual enable prompt on iOS.
      static Future<bool> isBluetoothEnabledTyped() async {
        final value = await _invoke<dynamic>('isBluetoothEnabled');
        return value == true;
      }

      /// Whether MLS material is initialized on the running protocol instance.
      static Future<bool> isMlsInitializedTyped() async {
        final value = await _invoke<dynamic>('isMlsInitialized');
        return value == true;
      }

      /// Whether this device is acting as a mesh relay (false when not initialized).
      static Future<bool> isRelayTyped() async {
        final value = await _invoke<dynamic>('isRelay');
        return value == true;
      }

      /// Whether a key package for [peerId] is queued locally.
      static Future<bool> hasPendingKeyPackageTyped(String peerId) async {
        final value = await _invoke<dynamic>('hasPendingKeyPackage', {'peerId': peerId});
        return value == true;
      }

      /// Device battery level when the platform exposes it; null otherwise.
      static Future<int?> getBatteryLevelTyped() async {
        final value = await _invoke<dynamic>('getBatteryLevel');
        if (value == null) return null;
        if (value is int) return value;
        if (value is num) return value.toInt();
        return int.tryParse(value.toString());
      }

      /// Local device address (`off1…`) without a map argument.
      static Future<String> localAddressTyped() async {
        final value = await _invoke<dynamic>('localAddress');
        return value?.toString() ?? '';
      }

      /// Pending ACK count for reliability UI (0 when the protocol is not running).
      static Future<int> getPendingAckCountTyped() async {
        final value = await _invoke<dynamic>('getPendingAckCount');
        if (value is int) return value;
        if (value is num) return value.toInt();
        return int.tryParse(value?.toString() ?? '') ?? 0;
      }

      /// Outbound retry queue depth (0 when the protocol is not running).
      static Future<int> getRetryQueueSizeTyped() async {
        final value = await _invoke<dynamic>('getRetryQueueSize');
        if (value is int) return value;
        if (value is num) return value.toInt();
        return int.tryParse(value?.toString() ?? '') ?? 0;
      }

      /// Connected BLE peer count (0 when BLE is not active).
      static Future<int> bleGetPeerCountTyped() async {
        final value = await _invoke<dynamic>('bleGetPeerCount');
        if (value is int) return value;
        if (value is num) return value.toInt();
        return int.tryParse(value?.toString() ?? '') ?? 0;
      }

      /// Active transport names from the core (empty when not initialized).
      static Future<List<String>> getActiveTransportsTyped() async {
        final value = await _invoke<dynamic>('getActiveTransports');
        if (value is List) {
          return value.map((e) => e.toString()).toList();
        }
        return const [];
      }
    """
)

FOOTER = TYPED_HELPERS + "}\n"


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


def render() -> str:
    parts = [HEADER]
    for m in METHODS:
        if m not in SKIP:
            parts.append(method_fn(m))
    parts.append(FOOTER)
    return "".join(parts)


def main() -> None:
    import sys

    rendered = render()
    if "--check" in sys.argv:
        if OUT.read_text() != rendered:
            print(f"{OUT.relative_to(REPO)} is stale; run {pathlib.Path(__file__).name}", file=sys.stderr)
            raise SystemExit(1)
        print(f"{OUT.name}: OK ({len(METHODS) - len(SKIP)} wrappers + typed helpers)")
        return
    OUT.write_text(rendered)
    print(f"Wrote {OUT}")


if __name__ == "__main__":
    main()
