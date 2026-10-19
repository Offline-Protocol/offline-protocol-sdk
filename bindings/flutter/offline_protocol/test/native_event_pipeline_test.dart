import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/src/native_event_pipeline.dart';
import 'package:offline_protocol/src/offline_protocol_event.dart';

void main() {
  test('in-order events dispatch without replay', () async {
    final pipeline = NativeEventPipeline(
      replayEvents: (_, __) async => [],
    );
    addTearDown(pipeline.close);

    final events = <OfflineProtocolNativeEvent>[];
    final sub = pipeline.stream.listen(events.add);

    pipeline.admit(OfflineProtocolNativeEvent(eventJson: '{"type":"a"}', seq: 0));
    pipeline.admit(OfflineProtocolNativeEvent(eventJson: '{"type":"b"}', seq: 1));

    await Future<void>.delayed(Duration.zero);
    await sub.cancel();
    expect(events.length, 2);
    expect(events[0].decodeEvent()['type'], 'a');
    expect(events[1].decodeEvent()['type'], 'b');
  });

  test('gap triggers replay and recovers missing seq', () async {
    final pipeline = NativeEventPipeline(
      replayEvents: (from, to) async {
        expect(from, 1);
        expect(to, 1);
        return [
          {'seq': 1, 'eventJson': '{"type":"middle"}'},
        ];
      },
    );
    addTearDown(pipeline.close);

    final types = <String>[];
    final sub = pipeline.stream.listen((event) {
      types.add(event.decodeEvent()['type'] as String);
    });

    pipeline.admit(OfflineProtocolNativeEvent(eventJson: '{"type":"first"}', seq: 0));
    pipeline.admit(OfflineProtocolNativeEvent(eventJson: '{"type":"third"}', seq: 2));

    await Future<void>.delayed(const Duration(milliseconds: 50));
    await sub.cancel();
    expect(
      types.where((t) => t != 'diagnostic').toList(),
      ['first', 'middle', 'third'],
    );
  });
}
