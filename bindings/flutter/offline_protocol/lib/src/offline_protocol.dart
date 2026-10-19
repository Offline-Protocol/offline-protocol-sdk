import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'offline_protocol_event.dart';

export 'offline_protocol_event.dart';

/// Host for the Offline Protocol engine on iOS and Android.
///
/// PR2 adds core event streaming, [sendMessage], and Internet transport enablement.
/// BLE, Wi‑Fi Direct, and lifecycle hardening ship in PR3.
class OfflineProtocol {
  OfflineProtocol._();

  static const MethodChannel _channel =
      MethodChannel('com.offlineprotocol/offline_protocol');

  static const EventChannel _eventsChannel =
      EventChannel('com.offlineprotocol/offline_protocol_events');

  static Map<String, dynamic>? _lastCreateConfig;

  /// Core protocol events (`message_received`, diagnostics, Internet status, …).
  ///
  /// Each event includes monotonic [OfflineProtocolNativeEvent.seq] when the
  /// native bridge supports gap detection (same contract as React Native).
  static Stream<OfflineProtocolNativeEvent> get events {
    return _eventsChannel.receiveBroadcastStream().map((dynamic raw) {
      if (raw is! Map) {
        throw StateError('Unexpected event payload: $raw');
      }
      final eventJson = raw['eventJson'];
      if (eventJson is! String) {
        throw StateError('Event missing eventJson string');
      }
      final seqRaw = raw['seq'];
      final int? seq = seqRaw is int ? seqRaw : (seqRaw as num?)?.toInt();
      return OfflineProtocolNativeEvent(eventJson: eventJson, seq: seq);
    });
  }

  /// Builds a [ProtocolConfig] map with all transports disabled for local
  /// engine smoke tests. Production apps should set transport flags explicitly.
  static Map<String, dynamic> smokeTestConfig({
    required String appId,
    required String profile,
  }) {
    return {
      'appId': appId,
      'profile': profile,
      'bleEnabled': false,
      'wifiDirectEnabled': false,
      'internetEnabled': false,
      'reticulumEnabled': false,
      'nostrEnabled': false,
    };
  }

  /// Creates the Rust engine from a JSON config object (camelCase or snake_case).
  ///
  /// Optional top-level `dors` and `relay` sections are applied the same way as
  /// on React Native after [ProtocolConfig] is constructed. When
  /// `transports.internet` is enabled with a `serverAddress`, [start] also
  /// calls [enableInternetTransport] (matching the React Native JS layer).
  static Future<void> createFromConfig(Map<String, dynamic> config) async {
    _lastCreateConfig = Map<String, dynamic>.from(config);
    await _channel.invokeMethod<void>('create', jsonEncode(config));
  }

  /// Initializes MLS with the platform secure storage providers.
  static Future<void> initializeMlsWithSecureStorage() async {
    await _channel.invokeMethod<void>('initializeMlsWithSecureStorage');
  }

  /// Starts the protocol core and auto-enables Internet when configured.
  static Future<void> start() async {
    await _channel.invokeMethod<void>('start');
    await _autoEnableInternetIfConfigured();
  }

  /// Stops the protocol core.
  static Future<void> stop() async {
    await _channel.invokeMethod<void>('stop');
  }

  /// Releases native resources. Safe to call more than once.
  static Future<void> destroy() async {
    _lastCreateConfig = null;
    await _channel.invokeMethod<void>('destroy');
  }

  /// This device's `off1…` address, or null before MLS is initialized.
  static Future<String?> localAddress() async {
    return _channel.invokeMethod<String>('localAddress');
  }

  /// Sends a message to [recipient] and returns the assigned message id.
  static Future<String> sendMessage({
    required String recipient,
    required String content,
    MessagePriority priority = MessagePriority.medium,
    String? replyToMsg,
  }) async {
    final id = await _channel.invokeMethod<String>('sendMessage', {
      'recipient': recipient,
      'content': content,
      'priority': priority.value,
      if (replyToMsg != null) 'replyToMsg': replyToMsg,
    });
    if (id == null || id.isEmpty) {
      throw StateError('sendMessage returned no message id');
    }
    return id;
  }

  /// Configures and starts the Internet relay transport.
  ///
  /// [config] uses the same shape as React Native `enableTransport('internet', …)`:
  /// `serverAddress`, optional `authToken`, `autoReconnect`, `port`, …
  static Future<void> enableInternetTransport(
    Map<String, dynamic> config,
  ) async {
    await _channel.invokeMethod<void>(
      'enableInternetTransport',
      jsonEncode(config),
    );
  }

  /// Convenience: create (smoke config) → MLS → start → read address.
  static Future<String?> bootstrapSmokeTest({
    required String appId,
    required String profile,
  }) async {
    await createFromConfig(smokeTestConfig(appId: appId, profile: profile));
    await initializeMlsWithSecureStorage();
    await start();
    return localAddress();
  }

  static Future<void> _autoEnableInternetIfConfigured() async {
    final root = _lastCreateConfig;
    if (root == null) return;

    final transports = root['transports'];
    if (transports is! Map) return;

    final internetRaw = transports['internet'];
    if (internetRaw is! Map) return;

    final internet = Map<String, dynamic>.from(internetRaw);
    final enabled = internet['enabled'] == true || root['internetEnabled'] == true;
    if (!enabled) return;

    final serverAddress = internet['serverAddress'] ?? internet['server_address'];
    if (serverAddress is! String || serverAddress.trim().isEmpty) {
      return;
    }

    try {
      await enableInternetTransport({
        ...internet,
        'enabled': true,
        'serverAddress': serverAddress,
        'autoReconnect': internet['autoReconnect'] ?? internet['auto_reconnect'] ?? true,
      });
    } catch (e, st) {
      // Same non-fatal contract as React Native `start()` auto-enable.
      debugPrint('[OfflineProtocol] Failed to auto-enable internet transport: $e\n$st');
    }
  }
}
