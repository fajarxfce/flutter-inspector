import 'dart:async';
import 'dart:convert';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:test/test.dart';

void main() {
  late InspectorBridge bridge;
  Future<dynamic> call(String method, [Json params = const {}]) => bridge
      .dispatch({'controller': 'test', 'method': method, 'params': params});
  setUp(() async {
    bridge = await InspectorBridge.start(maxBodyBytes: 128, maxSpoolBytes: 512);
    await call('hello');
  });
  tearDown(() async {
    await bridge.dispose();
  });
  test(
    'discovery reports availability without claiming or replacing a lease',
    () async {
      final info = await bridge.dispatch({'method': 'discover'});
      expect(info['busy'], true);
      expect(info['sessionId'], bridge.sessionId);
      expect(info.containsKey('events'), false);
      await call('detach');
      expect((await bridge.dispatch({'method': 'discover'}))['busy'], false);
      await bridge.dispatch({'controller': 'new', 'method': 'hello'});
      expect((await bridge.dispatch({'method': 'discover'}))['busy'], true);
      await expectLater(call('hello'), throwsStateError);
    },
  );
  test('one controller, frozen snapshots and bounded event history', () async {
    await expectLater(
      bridge.dispatch({'controller': 'other', 'method': 'hello'}),
      throwsStateError,
    );
    final data = <String, dynamic>{
      'nested': {'value': 1},
    };
    bridge.emit('test', data);
    (data['nested'] as Map)['value'] = 2;
    expect((await call('events'))['events'][0]['data']['nested']['value'], 1);
    for (var i = 0; i < 1005; i++) {
      bridge.emit('test', {'value': i});
    }
    final page = await call('events');
    expect(page['events'], hasLength(1000));
    expect(page['dropped'], 6);
  });
  test('capture bounds preserve truncation and chunk protocol', () async {
    final body = bridge.beginBody('application/octet-stream')!;
    body.add(List.generate(256, (i) => i));
    await body.close();
    expect(body.size, 128);
    expect(body.descriptor['complete'], false);
    final chunk = await call('body.read', {'id': body.id, 'offset': 64});
    expect(
      base64Decode(chunk['data'] as String),
      List.generate(64, (i) => i + 64),
    );
    await expectLater(
      call('body.read', {'id': body.id, 'offset': -1}),
      throwsArgumentError,
    );
  });
  test('detach releases original once; late decisions are rejected', () async {
    final pending = bridge.hold('request', {'id': 'one'});
    expect((await call('events'))['pending'], hasLength(1));
    await call('detach');
    expect((await pending)['action'], 'forward');
    await call('hello');
    await expectLater(
      call('network.decide', {'id': 'one', 'action': 'drop'}),
      throwsStateError,
    );
  });
  test('cancel and timeout clear held messages', () async {
    final cancel = Completer<void>();
    final pending = bridge.hold('response', {
      'id': 'one',
    }, cancelled: cancel.future);
    cancel.complete();
    expect((await pending)['action'], 'cancel');
    await call('intercept.rules', {
      'enabled': true,
      'host': '',
      'path': '',
      'method': '',
      'request': true,
      'response': true,
      'holdSeconds': 1,
    });
    final timed = bridge.hold('request', {'id': 'two'});
    expect(
      (await timed.timeout(const Duration(seconds: 3)))['reason'],
      'timeout',
    );
    expect((await call('events'))['pending'], isEmpty);
  });
  test('dispose makes API inert and permits a new bridge', () async {
    await bridge.dispose();
    expect(bridge.enabled, false);
    expect(bridge.beginBody(''), isNull);
    await expectLater(call('hello'), throwsStateError);
    bridge = await InspectorBridge.start();
    await call('hello');
    expect(bridge.connected, true);
  });
  test('concurrent initialization still creates one bridge', () async {
    await bridge.dispose();
    final first = InspectorBridge.start();
    await expectLater(InspectorBridge.start(), throwsStateError);
    bridge = await first;
    await call('hello');
    expect(bridge.connected, true);
  });
  test(
    'lost controller heartbeat releases original and allows takeover',
    () async {
      final pending = bridge.hold('request', {'id': 'lease'});
      expect(
        (await pending.timeout(const Duration(seconds: 18)))['reason'],
        'controller disconnected',
      );
      expect(bridge.connected, false);
      await bridge.dispatch({'controller': 'replacement', 'method': 'hello'});
      expect(bridge.connected, true);
    },
  );
}
