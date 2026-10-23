import 'dart:async';

import 'package:flutter/material.dart';
import 'package:offline_protocol/offline_protocol.dart';

void main() {
  runApp(const OfflineProtocolExampleApp());
}

class OfflineProtocolExampleApp extends StatelessWidget {
  const OfflineProtocolExampleApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Offline Protocol',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.teal),
        useMaterial3: true,
      ),
      home: const HomePage(),
    );
  }
}

class HomePage extends StatefulWidget {
  const HomePage({super.key});

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  String _status = 'Tap Start to boot the engine.';
  bool _busy = false;
  StreamSubscription<OfflineProtocolNativeEvent>? _eventSub;
  int _eventCount = 0;
  String? _lastEventType;

  @override
  void initState() {
    super.initState();
    // Subscribe before create/start so native assigns seq and replay can recover gaps.
    _eventSub = OfflineProtocol.events.listen((event) {
      if (!mounted) return;
      setState(() {
        _eventCount += 1;
        try {
          _lastEventType = event.decodeEvent()['type']?.toString();
        } catch (_) {
          _lastEventType = '(unparsed)';
        }
      });
    });
  }

  @override
  void dispose() {
    _eventSub?.cancel();
    super.dispose();
  }

  Future<void> _boot() async {
    setState(() {
      _busy = true;
      _status = 'Creating engine…';
      _eventCount = 0;
      _lastEventType = null;
    });
    try {
      final address = await OfflineProtocol.bootstrapSmokeTest(
        appId: 'com.offlineprotocol.flutter.example',
        profile: 'default',
      );
      if (!mounted) return;
      setState(() {
        _status = address == null
            ? 'Engine started but local address is not ready yet.'
            : 'This device: $address\nEvents received: $_eventCount'
                '${_lastEventType != null ? ' (last: $_lastEventType)' : ''}';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _status = 'Error: $e';
      });
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _teardown() async {
    setState(() {
      _busy = true;
      _status = 'Stopping…';
    });
    try {
      await OfflineProtocol.destroy();
      if (!mounted) return;
      setState(() {
        _status = 'Engine destroyed.';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _status = 'Teardown error: $e';
      });
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Offline Protocol'),
      ),
      body: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              _status,
              style: Theme.of(context).textTheme.bodyLarge,
            ),
            const SizedBox(height: 24),
            FilledButton(
              onPressed: _busy ? null : _boot,
              child: const Text('Start engine'),
            ),
            const SizedBox(height: 12),
            OutlinedButton(
              onPressed: _busy ? null : _teardown,
              child: const Text('Destroy engine'),
            ),
          ],
        ),
      ),
    );
  }
}
