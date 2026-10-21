import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/src/flutter_create_config_policy.dart';

void main() {
  test('allows BLE and Internet only configs', () {
    expect(
      () => FlutterCreateConfigPolicy.assertCreateConfigSupported({
        'appId': 'a',
        'profile': 'p',
        'bleEnabled': true,
        'internetEnabled': true,
      }),
      returnsNormally,
    );
  });

  test('refuses reticulumEnabled in camelCase or snake_case', () {
    for (final config in [
      {'reticulumEnabled': true},
      {'reticulum_enabled': true},
    ]) {
      expect(
        () => FlutterCreateConfigPolicy.assertCreateConfigSupported(config),
        throwsA(isA<UnsupportedError>()),
      );
    }
  });

  test('refuses nostrEnabled in camelCase or snake_case', () {
    for (final config in [
      {'nostrEnabled': true},
      {'nostr_enabled': true},
    ]) {
      expect(
        () => FlutterCreateConfigPolicy.assertCreateConfigSupported(config),
        throwsA(isA<UnsupportedError>()),
      );
    }
  });

  test('refuses enableTransport for reticulum and nostr', () {
    expect(
      () => FlutterCreateConfigPolicy.assertTransportTypeSupported('reticulum'),
      throwsA(isA<UnsupportedError>()),
    );
    expect(
      () => FlutterCreateConfigPolicy.assertTransportTypeSupported('Nostr'),
      throwsA(isA<UnsupportedError>()),
    );
    expect(
      () => FlutterCreateConfigPolicy.assertTransportTypeSupported('ble'),
      returnsNormally,
    );
  });
}
