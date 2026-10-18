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
}
