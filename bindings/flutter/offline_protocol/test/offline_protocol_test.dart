import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/offline_protocol.dart';

void main() {
  test('smokeTestConfig disables all transports', () {
    final config = OfflineProtocol.smokeTestConfig(
      appId: 'com.test.app',
      profile: 'user',
    );
    expect(config['appId'], 'com.test.app');
    expect(config['profile'], 'user');
    expect(config['bleEnabled'], isFalse);
    expect(config['internetEnabled'], isFalse);
    expect(config['nostrEnabled'], isFalse);
  });

  test('resolveInternetAutoEnableConfig nested transports.internet', () {
    final config = OfflineProtocol.resolveInternetAutoEnableConfig({
      'transports': {
        'internet': {
          'enabled': true,
          'serverAddress': 'relay.example.com',
          'authToken': 'secret',
        },
      },
    });
    expect(config, isNotNull);
    expect(config!['serverAddress'], 'relay.example.com');
    expect(config['authToken'], 'secret');
    expect(config['autoReconnect'], isTrue);
  });

  test('resolveInternetAutoEnableConfig legacy flat server key', () {
    final config = OfflineProtocol.resolveInternetAutoEnableConfig({
      'internetEnabled': true,
      'internetServerAddress': 'wss://legacy.example.com',
      'internetAutoReconnect': false,
    });
    expect(config, isNotNull);
    expect(config!['serverAddress'], 'wss://legacy.example.com');
    expect(config['autoReconnect'], isFalse);
  });

  test('resolveInternetAutoEnableConfig root authToken with legacy server', () {
    final config = OfflineProtocol.resolveInternetAutoEnableConfig({
      'internetEnabled': true,
      'internetServerAddress': 'relay.example.com',
      'authToken': 'root-secret',
    });
    expect(config, isNotNull);
    expect(config!['authToken'], 'root-secret');
  });

  test('resolveInternetAutoEnableConfig requires enabled flag', () {
    expect(
      OfflineProtocol.resolveInternetAutoEnableConfig({
        'transports': {
          'internet': {
            'serverAddress': 'relay.example.com',
          },
        },
      }),
      isNull,
    );
  });
}
