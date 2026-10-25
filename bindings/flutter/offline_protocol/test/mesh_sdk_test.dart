import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:offline_protocol/offline_protocol.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('com.offlineprotocol/offline_protocol');

  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (MethodCall call) async {
      switch (call.method) {
        case 'isMlsInitialized':
          return true;
        case 'internetIsReady':
          return false;
        case 'internetIsSuperseded':
          return true;
        case 'getState':
          return 'Running';
        case 'isRelay':
          return false;
        case 'hasPendingKeyPackage':
          return true;
        case 'getBatteryLevel':
          return 87;
        default:
          return null;
      }
    });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test('typed poll helpers coerce native values', () async {
    expect(await MeshSdk.isMlsInitializedTyped(), isTrue);
    expect(await MeshSdk.internetIsReadyTyped(), isFalse);
    expect(await MeshSdk.internetIsSupersededTyped(), isTrue);
    expect(await MeshSdk.getStateTyped(), 'Running');
    expect(await MeshSdk.isRelayTyped(), isFalse);
  });

  test('hasPendingKeyPackageTyped forwards peer id', () async {
    Object? capturedArgs;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (MethodCall call) async {
      if (call.method == 'hasPendingKeyPackage') {
        capturedArgs = call.arguments;
        return true;
      }
      return null;
    });
    expect(await MeshSdk.hasPendingKeyPackageTyped('peer-1'), isTrue);
    expect(capturedArgs, {'peerId': 'peer-1'});
  });

  test('getBatteryLevelTyped returns nullable int', () async {
    expect(await MeshSdk.getBatteryLevelTyped(), 87);
  });

  test('sendMessageTyped forwards fields and returns message id', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (MethodCall call) async {
      if (call.method == 'sendMessage') {
        expect(call.arguments, {
          'recipient': 'peer',
          'content': 'hi',
          'priority': 2,
          'replyToMsg': 'msg-1',
        });
        return 'out-id';
      }
      return null;
    });
    expect(
      await MeshSdk.sendMessageTyped(
        recipient: 'peer',
        content: 'hi',
        priority: 2,
        replyToMsg: 'msg-1',
      ),
      'out-id',
    );
  });

  test('a map-argument wrapper sends its own name and the map unchanged',
      () async {
    MethodCall? captured;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (MethodCall call) async {
      captured = call;
      return null;
    });
    final args = {
      'spaceId': 'space',
      'docId': 'doc',
      'collection': 'notes',
      'key': 'title',
      'valueJson': '"hello"',
    };
    await MeshSdk.dataMapSet(args);
    expect(captured?.method, 'dataMapSet');
    expect(captured?.arguments, args);
  });
}
