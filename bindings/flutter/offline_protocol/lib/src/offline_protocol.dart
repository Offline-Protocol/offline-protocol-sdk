import 'dart:convert';

import 'package:flutter/services.dart';

/// Host for the Offline Protocol engine on iOS and Android.
///
/// PR1 covers engine lifecycle only (create, MLS init, start, local address).
/// Transport bridges and event streaming follow in later PRs.
class OfflineProtocol {
  OfflineProtocol._();

  static const MethodChannel _channel =
      MethodChannel('com.offlineprotocol/offline_protocol');

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
  static Future<void> createFromConfig(Map<String, dynamic> config) async {
    await _channel.invokeMethod<void>('create', jsonEncode(config));
  }

  /// Initializes MLS with the platform secure storage providers.
  static Future<void> initializeMlsWithSecureStorage() async {
    await _channel.invokeMethod<void>('initializeMlsWithSecureStorage');
  }

  /// Starts the protocol core. Attach transports in a later PR before messaging.
  static Future<void> start() async {
    await _channel.invokeMethod<void>('start');
  }

  /// Stops the protocol core.
  static Future<void> stop() async {
    await _channel.invokeMethod<void>('stop');
  }

  /// Releases native resources. Safe to call more than once.
  static Future<void> destroy() async {
    await _channel.invokeMethod<void>('destroy');
  }

  /// This device's `off1…` address, or null before MLS is initialized.
  static Future<String?> localAddress() async {
    return _channel.invokeMethod<String>('localAddress');
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
}
