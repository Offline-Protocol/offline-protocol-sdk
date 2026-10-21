/// Preview policy: Reticulum and Nostr are not hosted on the Flutter plugin yet.
class FlutterCreateConfigPolicy {
  FlutterCreateConfigPolicy._();

  static const String unsupportedTransportMessage =
      'Reticulum and Nostr are not hosted in the Flutter plugin yet. '
      'Disable reticulumEnabled and nostrEnabled, or use React Native.';

  /// Throws [UnsupportedError] when [config] requests an unhosted transport.
  static void assertCreateConfigSupported(Map<String, dynamic> config) {
    if (_boolFlag(config, 'reticulumEnabled', 'reticulum_enabled')) {
      throw UnsupportedError(unsupportedTransportMessage);
    }
    if (_boolFlag(config, 'nostrEnabled', 'nostr_enabled')) {
      throw UnsupportedError(unsupportedTransportMessage);
    }
  }

  /// Throws [UnsupportedError] for runtime transport enable on unhosted types.
  static void assertTransportTypeSupported(String type) {
    switch (type.toLowerCase()) {
      case 'reticulum':
      case 'nostr':
        throw UnsupportedError(unsupportedTransportMessage);
      default:
        return;
    }
  }

  static bool _boolFlag(Map<String, dynamic> config, String camel, String snake) {
    final value = config[camel] ?? config[snake];
    return value == true;
  }
}
