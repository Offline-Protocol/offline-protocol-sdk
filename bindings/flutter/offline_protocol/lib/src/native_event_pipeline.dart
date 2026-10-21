import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import 'native_event_constants.dart';
import 'offline_protocol_event.dart';

typedef NativeEventReplayFn = Future<List<Map<String, dynamic>>> Function(
  int fromSeq,
  int toSeq,
);

/// Ordered delivery of native events with optional `seq` gap recovery (RN
/// `admitNativeEvent` parity).
class NativeEventPipeline {
  NativeEventPipeline({
    required NativeEventReplayFn replayEvents,
    void Function(String diagnosticJson)? onDiagnosticEvent,
  })  : _replayEvents = replayEvents,
        _onDiagnosticEvent = onDiagnosticEvent;

  final NativeEventReplayFn _replayEvents;
  final void Function(String diagnosticJson)? _onDiagnosticEvent;

  final StreamController<OfflineProtocolNativeEvent> _out =
      StreamController<OfflineProtocolNativeEvent>.broadcast();

  int? _lastEventSeq;
  _ReplayState? _replay;

  Stream<OfflineProtocolNativeEvent> get stream => _out.stream;

  void close() {
    _finishEventReplay();
    _out.close();
  }

  void reset() {
    _finishEventReplay();
    _lastEventSeq = null;
  }

  void admitRaw(dynamic raw) {
    final parsed = _parseNativeEvent(raw);
    if (parsed == null) return;
    admit(parsed);
  }

  void admit(OfflineProtocolNativeEvent event) {
    final seq = event.seq;
    if (seq == null) {
      _dispatchEventJson(event.eventJson);
      return;
    }

    final expected = _lastEventSeq == null ? seq : _lastEventSeq! + 1;
    if (seq < expected) {
      _finishEventReplay();
      _lastEventSeq = seq;
      _dispatchEventJson(event.eventJson);
      return;
    }
    _lastEventSeq = seq;

    if (seq > expected) {
      _requestEventReplay(expected, seq - 1);
    }

    final replay = _replay;
    if (replay == null) {
      _dispatchEventJson(event.eventJson);
      return;
    }
    replay.deferred.add(event);
    if (replay.deferred.length > maxDeferredNativeEvents) {
      _finishEventReplay();
    }
  }

  /// Asks native for `fromSeq..toSeq` and holds later events until it answers,
  /// the timer fires, or [maxDeferredNativeEvents] are held. Always leaves a
  /// replay in flight, so the caller defers the event that exposed the gap.
  void _requestEventReplay(int fromSeq, int toSeq) {
    var replay = _replay;
    if (replay == null) {
      replay = _ReplayState(
        timer: Timer(
          const Duration(milliseconds: nativeEventReplayTimeoutMs),
          _finishEventReplay,
        ),
      );
      _replay = replay;
    }
    replay.ranges.add([fromSeq, toSeq]);
    replay.outstanding += 1;

    final activeReplay = replay;
    _replayEvents(fromSeq, toSeq).then(
      (served) {
        if (_replay != activeReplay) return;
        for (final entry in served) {
          final seqRaw = entry['seq'];
          final json = entry['eventJson'];
          if (seqRaw is! int || json is! String) continue;
          if (seqRaw < fromSeq || seqRaw > toSeq) continue;
          activeReplay.recovered[seqRaw] = json;
        }
        activeReplay.outstanding -= 1;
        if (activeReplay.outstanding == 0) {
          _finishEventReplay();
        }
      },
      onError: (Object error, StackTrace st) {
        debugPrint('[OfflineProtocol] native event replay failed: $error\n$st');
        if (_replay == activeReplay) {
          activeReplay.outstanding -= 1;
          if (activeReplay.outstanding == 0) {
            _finishEventReplay();
          }
        }
      },
    );
  }

  void _finishEventReplay() {
    final replay = _replay;
    if (replay == null) return;
    _replay = null;
    replay.timer?.cancel();

    final present = Map<int, String>.from(replay.recovered);
    for (final event in replay.deferred) {
      final seq = event.seq;
      if (seq != null) {
        present[seq] = event.eventJson;
      }
    }

    if (replay.ranges.isEmpty && replay.deferred.isEmpty) {
      return;
    }

    final first = replay.ranges.isEmpty
        ? present.keys.reduce((a, b) => a < b ? a : b)
        : replay.ranges.map((r) => r[0]).reduce((a, b) => a < b ? a : b);
    final last = [
      if (present.isNotEmpty) present.keys.reduce((a, b) => a > b ? a : b),
      ...replay.ranges.map((r) => r[1]),
    ].reduce((a, b) => a > b ? a : b);

    for (final range in replay.ranges) {
      final from = range[0];
      final to = range[1];
      var recovered = 0;
      for (var seq = from; seq <= to; seq++) {
        if (replay.recovered.containsKey(seq)) recovered++;
      }
      if (recovered > 0) {
        _emitDiagnostic(
          level: 'info',
          message: 'native_event_replayed',
          context: {
            'from': from,
            'to': to,
            'recovered': recovered,
            'missing': to - from + 1 - recovered,
          },
        );
      }
    }

    // Linear in the seq span being flushed (same bound as React Native). Capped
    // in practice by [maxDeferredNativeEvents] and [nativeEventReplayCapacity].
    int? missingFrom;
    for (var seq = first; seq <= last; seq++) {
      final eventJson = present[seq];
      if (eventJson == null) {
        missingFrom ??= seq;
        continue;
      }
      if (missingFrom != null) {
        _reportEventGap(missingFrom, seq);
        missingFrom = null;
      }
      _dispatchEventJson(eventJson);
    }
    if (missingFrom != null) {
      _reportEventGap(missingFrom, last + 1);
    }
  }

  void _reportEventGap(int expected, int received) {
    final context = {
      'expected': expected,
      'received': received,
      'missing': received - expected,
    };
    debugPrint('[OfflineProtocol] native events lost before Dart: $context');
    _emitDiagnostic(
      level: 'warning',
      message: 'native_event_gap',
      context: context,
    );
  }

  void _emitDiagnostic({
    required String level,
    required String message,
    required Map<String, Object?> context,
  }) {
    final payload = jsonEncode({
      'type': 'diagnostic',
      'level': level,
      'message': message,
      'context': context,
    });
    _onDiagnosticEvent?.call(payload);
    _out.add(OfflineProtocolNativeEvent(eventJson: payload));
  }

  void _dispatchEventJson(String eventJson) {
    _out.add(OfflineProtocolNativeEvent(eventJson: eventJson));
  }

  static OfflineProtocolNativeEvent? _parseNativeEvent(dynamic raw) {
    try {
      if (raw is! Map) {
        throw FormatException('Expected a Map, got ${raw.runtimeType}');
      }
      final eventJson = raw['eventJson'];
      if (eventJson is! String) {
        throw FormatException('Missing eventJson string');
      }
      final seqRaw = raw['seq'];
      final int? seq = seqRaw is int ? seqRaw : (seqRaw as num?)?.toInt();
      return OfflineProtocolNativeEvent(eventJson: eventJson, seq: seq);
    } catch (e, st) {
      debugPrint('[OfflineProtocol] Dropping malformed native event: $e\n$st');
      return null;
    }
  }
}

class _ReplayState {
  _ReplayState({required this.timer});

  final List<List<int>> ranges = [];
  int outstanding = 0;
  final Map<int, String> recovered = {};
  final List<OfflineProtocolNativeEvent> deferred = [];
  Timer? timer;
}
