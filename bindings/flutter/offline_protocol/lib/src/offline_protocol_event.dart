import 'dart:convert';

/// One event from the native engine, as delivered on the event channel.
///
/// [eventJson] is the same opaque JSON string React Native receives on
/// `OfflineProtocol_Event`. Parse it with [decodeEvent] when you need fields.
class OfflineProtocolNativeEvent {
  const OfflineProtocolNativeEvent({
    required this.eventJson,
    this.seq,
  });

  final String eventJson;
  final int? seq;

  /// Parses [eventJson] into a map. Throws [FormatException] on invalid JSON.
  Map<String, dynamic> decodeEvent() {
    final decoded = jsonDecode(eventJson);
    if (decoded is! Map<String, dynamic>) {
      throw FormatException('Expected a JSON object', eventJson);
    }
    return decoded;
  }
}

/// Message priority for [OfflineProtocol.sendMessage]. Values match UniFFI.
enum MessagePriority {
  low(0),
  medium(1),
  high(2),
  critical(3);

  const MessagePriority(this.value);
  final int value;
}
