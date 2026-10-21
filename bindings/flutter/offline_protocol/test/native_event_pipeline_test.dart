import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/src/native_event_constants.dart';
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

  test('malformed native events are dropped, not dispatched', () async {
    final pipeline = NativeEventPipeline(replayEvents: (_, __) async => []);
    addTearDown(pipeline.close);

    final events = <OfflineProtocolNativeEvent>[];
    final sub = pipeline.stream.listen(events.add);

    pipeline.admitRaw('not a map');
    pipeline.admitRaw({'seq': 0});
    pipeline.admitRaw({'eventJson': 42, 'seq': 0});
    pipeline.admitRaw({'eventJson': '{"type":"ok"}', 'seq': 0});

    await Future<void>.delayed(Duration.zero);
    await sub.cancel();
    expect(events.map((e) => e.decodeEvent()['type']), ['ok']);
  });

  test('deferred cap flushes and reports the gap without waiting', () async {
    // A replay that never answers: only the cap can release the held events.
    final never = Completer<List<Map<String, dynamic>>>();
    final pipeline = NativeEventPipeline(replayEvents: (_, __) => never.future);
    addTearDown(pipeline.close);

    final types = <String>[];
    final messages = <String>[];
    final sub = pipeline.stream.listen((event) {
      final map = event.decodeEvent();
      if (map['type'] == 'diagnostic') {
        messages.add(map['message'] as String);
      } else {
        types.add(map['type'] as String);
      }
    });

    pipeline.admit(OfflineProtocolNativeEvent(eventJson: '{"type":"e0"}', seq: 0));
    // seq 1 is lost; everything from 2 is held behind the replay.
    for (var seq = 2; seq <= maxDeferredNativeEvents + 2; seq++) {
      pipeline.admit(
        OfflineProtocolNativeEvent(eventJson: '{"type":"e$seq"}', seq: seq),
      );
    }

    await Future<void>.delayed(Duration.zero);
    await sub.cancel();
    expect(types.length, maxDeferredNativeEvents + 2);
    expect(types.first, 'e0');
    expect(types[1], 'e2');
    expect(messages, contains('native_event_gap'));
  });

  test('reset after pause accepts contiguous seq without false gap', () async {
    var replayCalls = 0;
    final pipeline = NativeEventPipeline(
      replayEvents: (_, __) async {
        replayCalls += 1;
        return [];
      },
    );
    addTearDown(pipeline.close);

    final messages = <String>[];
    final sub = pipeline.stream.listen((event) {
      try {
        final map = event.decodeEvent();
        if (map['type'] == 'diagnostic') {
          messages.add(map['message'] as String? ?? '');
        } else {
          messages.add(map['type'] as String? ?? '');
        }
      } catch (_) {}
    });

    pipeline.admit(
      OfflineProtocolNativeEvent(eventJson: '{"type":"before_stop"}', seq: 4),
    );
    pipeline.reset();
    pipeline.admit(
      OfflineProtocolNativeEvent(eventJson: '{"type":"after_start"}', seq: 5),
    );
    pipeline.admit(
      OfflineProtocolNativeEvent(eventJson: '{"type":"after_start_2"}', seq: 6),
    );

    await Future<void>.delayed(Duration.zero);
    await sub.cancel();

    expect(replayCalls, 0);
    expect(messages, contains('after_start'));
    expect(messages, isNot(contains('native_event_gap')));
  });
}
