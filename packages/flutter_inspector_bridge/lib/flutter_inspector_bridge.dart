library;

import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'src/discovery.dart';

typedef Json = Map<String, dynamic>;
const inspectorDebugMode =
    !bool.fromEnvironment('dart.vm.product') &&
    !bool.fromEnvironment('dart.vm.profile');

abstract class InspectorProvider {
  String get id;
  String get kind;
  String get name;
  Future<dynamic> call(String method, Json params);
  Future<void> dispose();
}

class BodyCapture {
  BodyCapture(this.bridge, this.id, this.contentType, this.file)
    : _sink = file.openWrite() {
    unawaited(
      _sink.done.catchError((Object error) {
        _error = error;
        complete = false;
      }),
    );
  }
  final InspectorBridge bridge;
  final String id;
  final String contentType;
  final File file;
  final IOSink _sink;
  int size = 0;
  bool complete = true;
  bool _closed = false;
  Object? _error;
  bool get isClosed => _closed;

  Json get descriptor => {
    'id': id,
    'size': size,
    'complete': complete && _closed && _error == null,
    'contentType': contentType,
  };

  void add(List<int> data) {
    if (_closed || _error != null) return;
    final remaining = min(
      bridge.maxBodyBytes - size,
      bridge.maxSpoolBytes - bridge._spooledBytes,
    );
    if (remaining <= 0) {
      complete = false;
      return;
    }
    final take = min(remaining, data.length);
    try {
      _sink.add(data.sublist(0, take));
      size += take;
      bridge._spooledBytes += take;
    } catch (error) {
      _error = error;
      complete = false;
    }
    if (take < data.length) complete = false;
  }

  Future<void> close({bool interrupted = false}) async {
    if (_closed) return;
    _closed = true;
    complete = complete && !interrupted;
    try {
      await _sink.flush();
      await _sink.close();
    } catch (e) {
      _error = e;
      complete = false;
    }
    await bridge._finishBody(this);
  }
}

class InspectorBridge {
  InspectorBridge._(this.name, this.maxBodyBytes, this.maxSpoolBytes)
    : sessionId =
          DateTime.now().microsecondsSinceEpoch.toString() +
          '-' +
          Random.secure().nextInt(1 << 32).toRadixString(16);

  static InspectorBridge? _active;
  static bool _registered = false;
  static bool _starting = false;
  static Future<InspectorBridge> start({
    String name = 'Flutter app',
    int maxBodyBytes = 20 * 1024 * 1024,
    int maxSpoolBytes = 128 * 1024 * 1024,
    bool advertise = true,
  }) async {
    if (maxBodyBytes <= 0 || maxSpoolBytes < maxBodyBytes) {
      throw ArgumentError(
        'Spool limit must be at least the positive body limit',
      );
    }
    final bridge = InspectorBridge._(name, maxBodyBytes, maxSpoolBytes);
    if (!inspectorDebugMode) return bridge;
    if (_active != null || _starting) {
      throw StateError('One InspectorBridge is allowed per isolate');
    }
    _starting = true;
    try {
      bridge._directory = await Directory.systemTemp.createTemp(
        'flutter-inspector-',
      );
      _active = bridge;
    } finally {
      _starting = false;
    }
    if (!_registered) {
      developer.registerExtension('ext.flutter_inspector.call', (
        _,
        arguments,
      ) async {
        try {
          final active = _active;
          if (active == null) throw StateError('Inspector bridge is disposed');
          final payload = jsonDecode(arguments['payload'] ?? '{}') as Json;
          final result = await active.dispatch(payload);
          return developer.ServiceExtensionResponse.result(
            jsonEncode({'result': result}),
          );
        } catch (error) {
          return developer.ServiceExtensionResponse.result(
            jsonEncode({'error': error.toString()}),
          );
        }
      });
      _registered = true;
    }
    bridge._timer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => bridge._expire(),
    );
    if (advertise) {
      bridge._discovery = DiscoveryAdvertisement(
        directory: bridge._directory!,
        name: name,
        sessionId: bridge.sessionId,
        isolateId: developer.Service.getIsolateId(Isolate.current),
      )..start();
    }
    return bridge;
  }

  final String name;
  final String sessionId;
  final int maxBodyBytes;
  final int maxSpoolBytes;
  final _providers = <String, InspectorProvider>{};
  final _events = Queue<Json>();
  final _eventSizes = Queue<int>();
  int _eventBytes = 0;
  final _bodies = LinkedHashMap<String, BodyCapture>();
  final _activeBodies = <String, BodyCapture>{};
  int _spooledBytes = 0;
  final _holds = <String, _Hold>{};
  Directory? _directory;
  Timer? _timer;
  DiscoveryAdvertisement? _discovery;
  String? _controller;
  DateTime? _heartbeat;
  int _sequence = 0;
  int _bodySequence = 0;
  bool _disposed = false;
  Json rule = {
    'enabled': false,
    'host': '',
    'path': '',
    'method': '',
    'request': true,
    'response': false,
    'holdSeconds': 120,
  };

  bool get enabled => inspectorDebugMode && !_disposed && _directory != null;
  bool get connected => enabled && _controller != null;
  Iterable<InspectorProvider> get providers => _providers.values;

  void add(InspectorProvider provider) {
    if (!enabled) return;
    if (_providers.containsKey(provider.id)) {
      throw ArgumentError('Duplicate provider ID: ' + provider.id);
    }
    _providers[provider.id] = provider;
  }

  void emit(String kind, Json data) {
    if (!connected) return;
    // Freeze snapshots: request objects continue changing after this event.
    final encoded = jsonEncode(data);
    final size = utf8.encode(encoded).length;
    final copy = jsonDecode(encoded) as Json;
    final event = <String, dynamic>{
      'sequence': ++_sequence,
      'kind': kind,
      'data': copy,
    };
    _events.add(event);
    _eventSizes.add(size);
    _eventBytes += size;
    while (_events.length > 1000 || _eventBytes > 8 * 1024 * 1024) {
      _events.removeFirst();
      _eventBytes -= _eventSizes.removeFirst();
    }
    developer.postEvent('flutter_inspector.event', {
      'sequence': _sequence,
      'kind': kind,
    });
  }

  BodyCapture? beginBody(String contentType) {
    if (!connected || _activeBodies.length >= 64) return null;
    final id = sessionId + '-body-' + (++_bodySequence).toString();
    final capture = BodyCapture(
      this,
      id,
      contentType,
      File(_directory!.path + '/' + id),
    );
    _activeBodies[id] = capture;
    return capture;
  }

  Future<void> _finishBody(BodyCapture body) async {
    _activeBodies.remove(body.id);
    if (_disposed) {
      try {
        await body.file.delete();
      } on FileSystemException {
        /* disposed */
      }
      return;
    }
    _bodies[body.id] = body;
    while ((_spooledBytes > maxSpoolBytes - maxBodyBytes ||
            _bodies.length > 1000) &&
        _bodies.length > 1) {
      final oldest = _bodies.remove(_bodies.keys.first)!;
      _spooledBytes -= oldest.size;
      try {
        await oldest.file.delete();
      } on FileSystemException {
        /* already gone */
      }
    }
  }

  bool shouldHold(
    String direction,
    String method,
    Uri uri, {
    bool replay = false,
  }) {
    if (!connected ||
        replay ||
        rule['enabled'] != true ||
        rule[direction] != true) {
      return false;
    }
    final host = rule['host'] as String? ?? '';
    final path = rule['path'] as String? ?? '';
    final selectedMethod = rule['method'] as String? ?? '';
    return (host.isEmpty ||
            uri.host.toLowerCase().contains(host.toLowerCase())) &&
        (path.isEmpty || uri.path.contains(path)) &&
        (selectedMethod.isEmpty ||
            method.toUpperCase() == selectedMethod.toUpperCase());
  }

  Future<Json> hold(
    String direction,
    Json exchange, {
    Future<void>? cancelled,
  }) async {
    if (!connected) return {'action': 'forward'};
    final id = exchange['id'] as String;
    final seconds = (rule['holdSeconds'] as num? ?? 120).toInt().clamp(1, 600);
    final hold = _Hold(
      id,
      direction,
      exchange,
      DateTime.now().add(Duration(seconds: seconds)),
    );
    _holds[id] = hold;
    if (cancelled != null) {
      unawaited(
        cancelled.then(
          (_) {
            if (identical(_holds[id], hold)) {
              _complete(id, {'action': 'cancel'});
            }
          },
          onError: (Object _, StackTrace __) {
            if (identical(_holds[id], hold)) {
              _complete(id, {'action': 'cancel'});
            }
          },
        ),
      );
    }
    try {
      return await hold.completer.future;
    } finally {
      if (identical(_holds[id], hold)) _holds.remove(id);
    }
  }

  void _complete(String id, Json decision) {
    final hold = _holds.remove(id);
    if (hold == null || hold.completer.isCompleted) {
      throw StateError('Request is no longer held');
    }
    hold.completer.complete(decision);
  }

  void _release() {
    _controller = null;
    _heartbeat = null;
    rule['enabled'] = false;
    for (final id in _holds.keys.toList()) {
      _complete(id, {'action': 'forward', 'reason': 'controller disconnected'});
    }
  }

  void _expire() {
    if (_heartbeat != null &&
        DateTime.now().difference(_heartbeat!).inSeconds >= 15) {
      _release();
    }
    for (final hold in _holds.values.toList()) {
      if (DateTime.now().isAfter(hold.expires)) {
        _complete(hold.id, {'action': 'forward', 'reason': 'timeout'});
      }
    }
  }

  Future<dynamic> dispatch(Json payload) async {
    if (!enabled) throw StateError('Inspector is available only in debug mode');
    final method = payload['method'] as String? ?? '';
    final controller = payload['controller'] as String? ?? '';
    final params =
        (payload['params'] as Map?)?.cast<String, dynamic>() ??
        <String, dynamic>{};
    _expire();
    // Discovery never claims a controller lease or reveals captured data.
    if (method == 'discover') {
      return {
        'protocol': 1,
        'sessionId': sessionId,
        'name': name,
        'busy': connected,
      };
    }
    if (method == 'hello') {
      if (controller.isEmpty) throw ArgumentError('Controller ID is required');
      if (_controller != null && _controller != controller) {
        throw StateError('Another inspector controls this isolate');
      }
      _controller = controller;
      _heartbeat = DateTime.now();
      return {
        'protocol': 1,
        'sessionId': sessionId,
        'name': name,
        'providers': _providers.values
            .map((p) => {'id': p.id, 'kind': p.kind, 'name': p.name})
            .toList(),
      };
    }
    if (_controller != controller || controller.isEmpty) {
      throw StateError('Controller lease expired; reconnect');
    }
    switch (method) {
      case 'heartbeat':
        _heartbeat = DateTime.now();
        return {'alive': true};
      case 'detach':
        _release();
        return null;
      case 'events':
        final after = (params['after'] as num? ?? 0).toInt();
        final first = _events.isEmpty
            ? _sequence + 1
            : _events.first['sequence'] as int;
        return {
          'cursor': _sequence,
          'dropped': max(0, first - after - 1),
          'events': _events
              .where((e) => (e['sequence'] as int) > after)
              .toList(),
          'pending': _holds.values.map((h) => h.toJson()).toList(),
        };
      case 'body.read':
        final body = _bodies[params['id']];
        if (body == null) {
          throw StateError('Body expired from the app capture cache');
        }
        final offset = (params['offset'] as num? ?? 0).toInt();
        if (offset < 0 || offset > body.size) {
          throw ArgumentError('Invalid body offset');
        }
        final file = await body.file.open();
        try {
          await file.setPosition(offset);
          final bytes = await file.read(min(65536, body.size - offset));
          return {
            'data': base64Encode(bytes),
            'size': body.size,
            'complete': body.complete,
          };
        } finally {
          await file.close();
        }
      case 'intercept.rules':
        final seconds = params['holdSeconds'];
        if (seconds is! num || seconds < 1 || seconds > 600) {
          throw ArgumentError('Hold timeout must be 1–600 seconds');
        }
        for (final key in ['enabled', 'request', 'response']) {
          if (params[key] is! bool) {
            throw ArgumentError('Expected boolean rule field: ' + key);
          }
        }
        for (final key in ['host', 'path', 'method']) {
          if (params[key] is! String) {
            throw ArgumentError('Expected string rule field: ' + key);
          }
        }
        rule = Map.of(params);
        return rule;
      case 'network.decide':
        final action = params['action'];
        if (!['forward', 'drop', 'mock'].contains(action)) {
          throw ArgumentError('Unknown intercept action');
        }
        _complete(params['id'] as String, params);
        return null;
      default:
        final provider = _providers[params['provider']];
        if (provider == null) {
          throw ArgumentError('Provider is not registered in this isolate');
        }
        return provider.call(method, params);
    }
  }

  Future<void> dispose() async {
    if (_disposed) return;
    _release();
    _disposed = true;
    _timer?.cancel();
    await _discovery?.dispose();
    for (final capture in _activeBodies.values.toList()) {
      await capture.close(interrupted: true);
    }
    for (final provider in _providers.values) {
      await provider.dispose();
    }
    _providers.clear();
    _events.clear();
    _eventSizes.clear();
    _eventBytes = 0;
    if (identical(_active, this)) _active = null;
    try {
      await _directory?.delete(recursive: true);
    } on FileSystemException {
      /* capture may be closing */
    }
  }
}

class _Hold {
  _Hold(this.id, this.direction, this.exchange, this.expires);
  final String id;
  final String direction;
  final Json exchange;
  final DateTime expires;
  final completer = Completer<Json>();
  Json toJson() => {
    'id': id,
    'direction': direction,
    'expiresAt': expires.millisecondsSinceEpoch,
    'exchange': exchange,
  };
}
