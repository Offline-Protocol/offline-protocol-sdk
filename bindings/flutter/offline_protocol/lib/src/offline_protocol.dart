import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'native_event_pipeline.dart';
import 'offline_protocol_event.dart';

export 'native_event_constants.dart';
export 'offline_protocol_event.dart';

/// Host for the Offline Protocol engine on iOS and Android (lifecycle, events,
/// messaging, Internet, BLE, Wi‑Fi Direct, native event replay).
class OfflineProtocol {
  OfflineProtocol._();

  static const MethodChannel _channel =
      MethodChannel('com.offlineprotocol/offline_protocol');

  static const EventChannel _eventsChannel =
      EventChannel('com.offlineprotocol/offline_protocol_events');

  static Map<String, dynamic>? _lastCreateConfig;
  static NativeEventPipeline? _eventPipeline;
  static StreamSubscription<dynamic>? _nativeEventSubscription;

  /// Core protocol events (`message_received`, diagnostics, Internet status, …).
  ///
  /// Subscribe **before** [createFromConfig] if you need every event and gap
  /// recovery: native code only assigns `seq` and fills the replay ring for
  /// events delivered while a listener is attached (same contract as React
  /// Native). Events emitted before you listen are not replayable.
  ///
  /// Delivery uses the same `seq` / gap recovery contract as React Native
  /// (`admitNativeEvent` + native `replayEvents`).
  static Stream<OfflineProtocolNativeEvent> get events {
    _ensureEventPipeline();
    return _eventPipeline!.stream;
  }

  /// Minimal [ProtocolConfig] for local engine smoke tests.
  ///
  /// The core requires at least one transport flag; Internet is enabled so the
  /// example can boot without starting BLE (iOS needs `bluetooth-central` /
  /// `bluetooth-peripheral` background modes before [start] may call
  /// `BleManager.start()`). No relay is configured, so the Internet transport
  /// is not auto-started. Production apps should set flags explicitly.
  static Map<String, dynamic> smokeTestConfig({
    required String appId,
    required String profile,
  }) {
    return {
      'appId': appId,
      'profile': profile,
      'bleEnabled': false,
      'wifiDirectEnabled': false,
      'internetEnabled': true,
      'reticulumEnabled': false,
      'nostrEnabled': false,
    };
  }

  /// Creates the Rust engine from a JSON config object (camelCase or snake_case).
  ///
  /// Optional top-level `dors` and `relay` sections are applied the same way as
  /// on React Native after [ProtocolConfig] is constructed. When Internet or
  /// Wi‑Fi Direct are configured, [start] auto-enables them (React Native JS layer).
  ///
  /// Throws a [PlatformException] when `reticulumEnabled` or `nostrEnabled` is
  /// true: the plugin hosts no manager for either, and the core would otherwise
  /// route messages to a transport nothing serves.
  ///
  /// Resets Dart `seq` tracking to match the native session (native [create]
  /// tears down the previous engine and restarts event numbering from zero).
  static Future<void> createFromConfig(Map<String, dynamic> config) async {
    _lastCreateConfig = Map<String, dynamic>.from(config);
    _ensureEventPipeline();
    _eventPipeline!.reset();
    await _channel.invokeMethod<void>('create', jsonEncode(config));
  }

  /// Initializes MLS with the platform secure storage providers.
  static Future<void> initializeMlsWithSecureStorage() async {
    await _channel.invokeMethod<void>('initializeMlsWithSecureStorage');
  }

  /// Starts the protocol core and auto-enables configured transports.
  static Future<void> start() async {
    await _channel.invokeMethod<void>('start');
    await _autoEnableTransportsIfConfigured();
  }

  /// Stops the protocol core and active transports.
  ///
  /// Resets Dart `seq` tracking so a later [start] on the same engine instance
  /// stays aligned with native numbering (native does not rewind `seq` on stop).
  static Future<void> stop() async {
    await _channel.invokeMethod<void>('stop');
    _ensureEventPipeline();
    _eventPipeline!.reset();
  }

  /// Releases native resources. Safe to call more than once.
  ///
  /// Clears Dart event state and drops the EventChannel subscription; the next
  /// [createFromConfig] or access to [events] reattaches it. Your existing
  /// [StreamSubscription] on [events] remains valid (same broadcast stream).
  static Future<void> destroy() async {
    _lastCreateConfig = null;
    await _channel.invokeMethod<void>('destroy');
    _detachNativeEventSubscription(resetPipeline: true);
  }

  /// Method-channel entry points implemented by the Flutter plugin (pinned in tests).
  @visibleForTesting
  static const List<String> nativeMethodChannelMethods = [
    'create',
    'initializeMlsWithSecureStorage',
    'start',
    'stop',
    'destroy',
    'localAddress',
    'sendMessage',
    'enableInternetTransport',
    'enableTransport',
    'replayEvents',
  ];

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

  /// Enables a transport by name (`internet`, `ble`, `wifiDirect`, …).
  ///
  /// [config] uses the same JSON shape as React Native `enableTransport`.
  static Future<void> enableTransport(
    String type,
    Map<String, dynamic> config,
  ) async {
    await _channel.invokeMethod<void>('enableTransport', {
      'type': type,
      'config': jsonEncode(config),
    });
  }

  /// Configures and starts the Internet relay transport.
  ///
  /// [config] uses the same shape as React Native `enableTransport('internet', …)`:
  /// `serverAddress`, optional `authToken`, `autoReconnect`, `port`, …
  static Future<void> enableInternetTransport(
    Map<String, dynamic> config,
  ) async {
    await enableTransport('internet', config);
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

  static void _ensureEventPipeline() {
    _eventPipeline ??= NativeEventPipeline(
      replayEvents: _replayEventsFromNative,
    );
    if (_nativeEventSubscription != null) return;
    _nativeEventSubscription = _eventsChannel.receiveBroadcastStream().listen(
      (raw) {
        _eventPipeline?.admitRaw(raw);
      },
      // Without a handler, an error from the native stream handler is an
      // unhandled error in the root zone. Events keep flowing after it.
      onError: (Object error, StackTrace st) {
        debugPrint('[OfflineProtocol] native event channel error: $error\n$st');
      },
    );
  }

  /// Cancels the EventChannel listener and optionally clears pipeline seq state.
  static void _detachNativeEventSubscription({required bool resetPipeline}) {
    _nativeEventSubscription?.cancel();
    _nativeEventSubscription = null;
    if (resetPipeline) {
      _eventPipeline?.reset();
    }
  }

  static Future<List<Map<String, dynamic>>> _replayEventsFromNative(
    int fromSeq,
    int toSeq,
  ) async {
    final result = await _channel.invokeMethod<List<dynamic>>('replayEvents', {
      'fromSeq': fromSeq,
      'toSeq': toSeq,
    });
    if (result == null) return const [];
    final out = <Map<String, dynamic>>[];
    for (final entry in result) {
      if (entry is! Map) continue;
      final seqRaw = entry['seq'];
      final json = entry['eventJson'];
      if (json is! String) continue;
      final int? seq = seqRaw is int
          ? seqRaw
          : seqRaw is num
              ? seqRaw.toInt()
              : null;
      if (seq == null) continue;
      out.add({'seq': seq, 'eventJson': json});
    }
    return out;
  }

  /// Mirrors the React Native JavaScript `start()` auto-enable
  /// (`transports.internet.enabled` with a `serverAddress`), and also accepts
  /// the flat `internetEnabled` / `internetServerAddress` spellings the native
  /// config parsers take. Exposed for unit tests only.
  @visibleForTesting
  static Map<String, dynamic>? resolveInternetAutoEnableConfig(
    Map<String, dynamic> root,
  ) =>
      _internetAutoEnableConfig(root);

  /// Wi‑Fi Direct / peer-stream auto-enable after [start]. Exposed for tests.
  @visibleForTesting
  static Map<String, dynamic>? resolveWifiDirectAutoEnableConfig(
    Map<String, dynamic> root,
  ) =>
      _wifiDirectAutoEnableConfig(root);

  static Map<String, dynamic>? _internetAutoEnableConfig(Map<String, dynamic> root) {
    final transports = root['transports'];
    Map<String, dynamic> internet = {};
    final hasNestedInternet = transports is Map && transports['internet'] is Map;
    if (hasNestedInternet) {
      internet = Map<String, dynamic>.from(transports['internet'] as Map);
    }

    final serverRaw = internet['serverAddress'] ??
        internet['server_address'] ??
        root['internetServerAddress'] ??
        root['internet_server_address'];
    if (serverRaw is! String || serverRaw.trim().isEmpty) {
      return null;
    }

    final nestedEnabled = hasNestedInternet && internet['enabled'] == true;
    final topLevelEnabled = root['internetEnabled'] == true;
    if (!nestedEnabled && !topLevelEnabled) {
      return null;
    }

    final autoReconnect = internet['autoReconnect'] ??
        internet['auto_reconnect'] ??
        root['internetAutoReconnect'] ??
        root['internet_auto_reconnect'] ??
        true;

    final enableConfig = <String, dynamic>{
      ...internet,
      'enabled': true,
      'serverAddress': serverRaw.trim(),
      'autoReconnect': autoReconnect,
    };
    final auth = internet['authToken'] ??
        internet['auth_token'] ??
        root['authToken'] ??
        root['auth_token'];
    if (auth is String && auth.isNotEmpty) {
      enableConfig['authToken'] = auth;
    }
    return enableConfig;
  }

  static Map<String, dynamic>? _wifiDirectAutoEnableConfig(
    Map<String, dynamic> root,
  ) {
    final transports = root['transports'];
    if (transports is! Map || transports['wifiDirect'] is! Map) {
      if (root['wifiDirectEnabled'] != true) {
        return null;
      }
      return <String, dynamic>{'enabled': true};
    }
    final wifi = Map<String, dynamic>.from(transports['wifiDirect'] as Map);
    if (wifi['enabled'] != true && root['wifiDirectEnabled'] != true) {
      return null;
    }
    return {...wifi, 'enabled': true};
  }

  static Future<void> _autoEnableTransportsIfConfigured() async {
    final root = _lastCreateConfig;
    if (root == null) return;

    final internetConfig = _internetAutoEnableConfig(root);
    if (internetConfig != null) {
      try {
        await enableInternetTransport(internetConfig);
      } catch (e, st) {
        debugPrint(
          '[OfflineProtocol] Failed to auto-enable internet transport: $e\n$st',
        );
      }
    }

    final wifiConfig = _wifiDirectAutoEnableConfig(root);
    if (wifiConfig != null) {
      try {
        await enableTransport('wifiDirect', wifiConfig);
      } catch (e, st) {
        debugPrint(
          '[OfflineProtocol] Failed to auto-enable Wi-Fi Direct transport: $e\n$st',
        );
      }
    }
  }
}
