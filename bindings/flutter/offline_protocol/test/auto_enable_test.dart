import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/offline_protocol.dart';

/// `start()` auto-enables each configured transport in turn. One that fails
/// must not stop the next: nothing retries an auto-enable, so a skipped
/// transport stays off for the whole session with no error reaching the app.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('com.offlineprotocol/offline_protocol');
  const events = MethodChannel('com.offlineprotocol/offline_protocol_events');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  late List<String> calls;

  setUp(() {
    calls = [];
    // The event channel's listen/cancel, so `createFromConfig` can subscribe.
    messenger.setMockMethodCallHandler(events, (call) async => null);
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'enableTransport') {
        final type = (call.arguments as Map)['type'] as String;
        calls.add('enableTransport:$type');
        if (type == 'internet') {
          throw PlatformException(code: 'ERROR', message: 'relay unreachable');
        }
        return null;
      }
      calls.add(call.method);
      return null;
    });
  });

  tearDown(() async {
    await OfflineProtocol.destroy();
    messenger.setMockMethodCallHandler(channel, null);
    messenger.setMockMethodCallHandler(events, null);
  });

  test('a failing transport does not skip the ones after it', () async {
    await OfflineProtocol.createFromConfig({
      'appId': 'app',
      'profile': 'profile',
      'wifiDirectEnabled': true,
      'transports': {
        'internet': {'enabled': true, 'serverAddress': 'relay.example.com'},
        'nostr': {
          'enabled': true,
          'relayUrls': ['wss://relay.example.com'],
        },
        'reticulum': {'enabled': true, 'daemonAddress': '10.0.0.2:4242'},
      },
    });
    await OfflineProtocol.start();

    expect(calls, [
      'create',
      'start',
      'enableTransport:internet',
      'enableTransport:nostr',
      'enableTransport:reticulum',
      'enableTransport:wifiDirect',
    ]);
  });

  test('nothing is auto-enabled when no transport section asks for it',
      () async {
    await OfflineProtocol.createFromConfig(
      OfflineProtocol.minimalFootprintConfig(appId: 'app', profile: 'p'),
    );
    await OfflineProtocol.start();

    expect(calls, ['create', 'start']);
  });
}
