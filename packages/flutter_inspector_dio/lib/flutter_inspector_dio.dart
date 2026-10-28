library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:dio/dio.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';

List<Json> _headers(Map<String, dynamic> source) => [
  for (final entry in source.entries)
    for (final value
        in (entry.value is List ? entry.value as List : [entry.value]))
      {'name': entry.key, 'value': value.toString()},
];
Map<String, dynamic> _headerMap(dynamic value) {
  final result = <String, dynamic>{};
  for (final item in value as List? ?? []) {
    final h = (item as Map).cast<String, dynamic>();
    final name = (h['name'] as String).toLowerCase();
    final text = h['value'] as String;
    if (name.contains(RegExp(r'[\r\n:]')) || text.contains(RegExp(r'[\r\n]'))) {
      throw ArgumentError('Invalid header');
    }
    if (result.containsKey(name)) {
      final previous = result[name];
      result[name] = [
        ...(previous is List ? previous : [previous]),
        text,
      ];
    } else {
      result[name] = text;
    }
  }
  return result;
}

Map<String, List<String>> _responseHeaders(dynamic value) =>
    _headerMap(value).map(
      (key, value) => MapEntry(
        key,
        value is List ? value.cast<String>() : [value as String],
      ),
    );

class DioInspector implements InspectorProvider {
  DioInspector({
    required this.id,
    required this.dio,
    required this.bridge,
    String? name,
  }) : name = name ?? id,
       _original = dio.httpClientAdapter {
    if (bridge.enabled) {
      if (_attached[dio] != null) {
        throw StateError('This Dio instance already has an inspector');
      }
      bridge.add(this);
      _attached[dio] = this;
      _interceptor = _CaptureInterceptor(this);
      dio.interceptors.add(_interceptor!);
      _adapter = _InspectingAdapter(this, _original);
      dio.httpClientAdapter = _adapter!;
    }
  }
  @override
  final String id;
  @override
  final String name;
  final Dio dio;
  final InspectorBridge bridge;
  final HttpClientAdapter _original;
  _InspectingAdapter? _adapter;
  _CaptureInterceptor? _interceptor;
  static final _attached = Expando<DioInspector>();
  final _records = Expando<_Capture>();
  final _replays = <String, CancelToken>{};
  int _sequence = 0;
  @override
  String get kind => 'dio';

  @override
  Future<dynamic> call(String method, Json params) async {
    if (method == 'replay.cancel') {
      _replays[params['id']]?.cancel('Cancelled from Inspector');
      return null;
    }
    if (method != 'replay') throw ArgumentError('Unsupported Dio operation');
    final replayId = params['id'] as String;
    if (_replays.containsKey(replayId)) {
      throw StateError('This request is already running');
    }
    final token = CancelToken();
    _replays[replayId] = token;
    try {
      final data = params['bodyBase64'] == true
          ? base64Decode(params['body'] as String? ?? '')
          : utf8.encode(params['body'] as String? ?? '');
      if (data.length > bridge.maxBodyBytes) {
        throw ArgumentError('Replay body exceeds capture limit');
      }
      final headers = _headerMap(params['headers']);
      headers.removeWhere((key, _) => key.toLowerCase() == 'content-length');
      final response = await dio.request<dynamic>(
        params['url'] as String,
        data: data.isEmpty ? null : Uint8List.fromList(data),
        cancelToken: token,
        options: Options(
          method: params['method'] as String,
          headers: headers,
          extra: {
            'flutterInspectorReplay': true,
            'flutterInspectorRunId': params['draftId'] != null
                ? replayId
                : null,
            'flutterInspectorDraftId': params['draftId'],
          },
          followRedirects: params['followRedirects'] == true,
          validateStatus: (_) => true,
          responseType: ResponseType.bytes,
        ),
      );
      return {
        'status': response.statusCode,
        'exchange': record(response.requestOptions).exchange,
      };
    } finally {
      _replays.remove(replayId);
    }
  }

  @override
  Future<void> dispose() async {
    for (final token in _replays.values) {
      token.cancel('Inspector disposed');
    }
    if (_interceptor != null) dio.interceptors.remove(_interceptor);
    if (identical(_attached[dio], this)) _attached[dio] = null;
    if (identical(dio.httpClientAdapter, _adapter)) {
      dio.httpClientAdapter = _original;
    }
  }

  _Capture record(RequestOptions options) =>
      _records[options] ??= _Capture(this, options);
}

class _Capture {
  _Capture(this.owner, RequestOptions options) : started = DateTime.now() {
    exchange = {
      'id':
          options.extra['flutterInspectorRunId'] ??
          owner.bridge.sessionId +
              '-' +
              owner.id +
              '-' +
              (++owner._sequence).toString(),
      if (options.extra['flutterInspectorDraftId'] != null)
        'repeaterId': options.extra['flutterInspectorDraftId'],
      'sessionId': owner.bridge.sessionId,
      'clientId': owner.id,
      'startedAt': started.millisecondsSinceEpoch,
      'durationMs': 0,
      'source': options.extra['flutterInspectorReplay'] == true
          ? 'repeater_app'
          : 'app',
      'state': 'pending',
      'request': {
        'method': options.method,
        'url': options.uri.toString(),
        'headers': _headers(options.headers),
      },
    };
  }
  final DioInspector owner;
  final DateTime started;
  late final Json exchange;
  void emit() {
    exchange['durationMs'] = DateTime.now().difference(started).inMilliseconds;
    owner.bridge.emit('network', exchange);
  }

  void fail(Object error) {
    exchange['state'] =
        error is DioException && error.type == DioExceptionType.cancel
        ? (error.error == 'inspector_drop' ? 'dropped' : 'cancelled')
        : 'error';
    exchange['error'] = error.toString();
    emit();
  }
}

class _CaptureInterceptor extends Interceptor {
  _CaptureInterceptor(this.owner);
  final DioInspector owner;
  @override
  void onRequest(RequestOptions options, RequestInterceptorHandler handler) {
    if (owner.bridge.connected) owner.record(options).emit();
    handler.next(options);
  }

  @override
  void onError(DioException error, ErrorInterceptorHandler handler) {
    owner._records[error.requestOptions]?.fail(error);
    handler.next(error);
  }
}

class _InspectingAdapter implements HttpClientAdapter {
  _InspectingAdapter(this.owner, this.inner);
  final DioInspector owner;
  final HttpClientAdapter inner;
  @override
  void close({bool force = false}) => inner.close(force: force);

  @override
  Future<ResponseBody> fetch(
    RequestOptions options,
    Stream<Uint8List>? requestStream,
    Future<void>? cancelFuture,
  ) async {
    final bridge = owner.bridge;
    if (!bridge.connected) {
      return inner.fetch(options, requestStream, cancelFuture);
    }
    final record = owner.record(options);
    final exchange = record.exchange;
    final emit = record.emit;
    final request = exchange['request'] as Json;
    // Dio has now transformed the body and added multipart boundaries / length.
    request['method'] = options.method;
    request['url'] = options.uri.toString();
    request['headers'] = _headers(options.headers);
    final replay = options.extra['flutterInspectorReplay'] == true;
    var outgoing = requestStream;
    BodyCapture? requestCapture;
    try {
      requestCapture = outgoing == null
          ? null
          : bridge.beginBody(options.contentType ?? '');
      if (outgoing != null && requestCapture != null) {
        final capture = requestCapture;
        outgoing = _tap(outgoing, capture, () {
          request['body'] = capture.descriptor;
          emit();
        });
      }
      if (outgoing != null && requestCapture == null) {
        request['body'] = {
          'id': exchange['id'] + '-unavailable-request',
          'size': 0,
          'complete': false,
          'contentType': options.contentType ?? '',
        };
      }
      emit();
      if (options.data is! Stream &&
          bridge.shouldHold(
            'request',
            options.method,
            options.uri,
            replay: replay,
          )) {
        final buffered = await _buffer(
          outgoing,
          bridge.maxBodyBytes,
          cancelFuture,
          options,
        );
        outgoing = buffered.stream;
        if (buffered.complete) {
          exchange['state'] = 'held_request';
          emit();
          final decision = await bridge.hold(
            'request',
            exchange,
            cancelled: cancelFuture,
          );
          _checkDecision(decision, options);
          if (decision['action'] == 'mock') {
            return await _mock(decision, options, exchange, emit);
          }
          if (decision['url'] is String) {
            final uri = Uri.parse(decision['url'] as String);
            if (!['http', 'https'].contains(uri.scheme) || uri.host.isEmpty) {
              throw ArgumentError('Expected HTTP(S) URL');
            }
            options.path = decision['url'] as String;
            options.baseUrl = '';
            options.queryParameters.clear();
          }
          if (decision['method'] is String) {
            options.method = decision['method'] as String;
          }
          if (decision['headers'] is List) {
            final length = options.headers[Headers.contentLengthHeader];
            final headers = _headerMap(decision['headers']);
            headers.removeWhere(
              (key, _) => key.toLowerCase() == 'content-length',
            );
            if (length != null) headers[Headers.contentLengthHeader] = length;
            options.headers = headers;
          }
          if (decision['body'] is String) {
            final bytes = decision['bodyBase64'] == true
                ? base64Decode(decision['body'] as String)
                : utf8.encode(decision['body'] as String);
            if (bytes.length > bridge.maxBodyBytes) {
              throw ArgumentError('Edited body exceeds capture limit');
            }
            outgoing = Stream.value(Uint8List.fromList(bytes));
            final headers = Map<String, dynamic>.of(options.headers);
            headers.removeWhere((k, _) => k.toLowerCase() == 'content-length');
            headers[Headers.contentLengthHeader] = bytes.length;
            options.headers = headers;
            final edited = bridge.beginBody(options.contentType ?? '');
            if (edited != null) {
              edited.add(bytes);
              await edited.close();
              request['body'] = edited.descriptor;
            }
          }
          request['method'] = options.method;
          request['url'] = options.uri.toString();
          request['headers'] = _headers(options.headers);
          exchange['state'] = 'pending';
          emit();
        }
      }
      var response = await inner.fetch(options, outgoing, cancelFuture);
      final responseData = <String, dynamic>{
        'status': response.statusCode,
        'headers': _headers(response.headers),
      };
      exchange['response'] = responseData;
      final type = response.headers[Headers.contentTypeHeader]?.join(';') ?? '';
      final originalStream = response.stream;
      Stream<Uint8List> observedResponse() async* {
        final capture = bridge.beginBody(type);
        if (capture != null) {
          yield* _tap(originalStream, capture, () {
            responseData['body'] = capture.descriptor;
            if (exchange['state'] != 'cancelled' &&
                exchange['state'] != 'error') {
              exchange['state'] = 'complete';
            }
            emit();
          }, onError: record.fail);
        } else {
          responseData['body'] = {
            'id': exchange['id'] + '-unavailable-response',
            'size': 0,
            'complete': false,
            'contentType': type,
          };
          yield* originalStream;
          exchange['state'] = 'complete';
          emit();
        }
      }

      response.stream = observedResponse();
      if (options.responseType != ResponseType.stream &&
          !type.contains('text/event-stream') &&
          bridge.shouldHold(
            'response',
            options.method,
            options.uri,
            replay: replay,
          )) {
        final buffered = await _buffer(
          response.stream,
          bridge.maxBodyBytes,
          cancelFuture,
          options,
        );
        response.stream = buffered.stream!;
        if (buffered.complete) {
          exchange['state'] = 'held_response';
          emit();
          final decision = await bridge.hold(
            'response',
            exchange,
            cancelled: cancelFuture,
          );
          _checkDecision(decision, options);
          if (decision['status'] is num) {
            response.statusCode = _status(decision);
          }
          if (decision['headers'] is List) {
            response.headers = _responseHeaders(decision['headers']);
          }
          if (decision['body'] is String) {
            final bytes = decision['bodyBase64'] == true
                ? base64Decode(decision['body'] as String)
                : utf8.encode(decision['body'] as String);
            if (bytes.length > bridge.maxBodyBytes) {
              throw ArgumentError('Edited body exceeds capture limit');
            }
            response.stream = Stream.value(Uint8List.fromList(bytes));
            response.headers.removeWhere(
              (k, _) => [
                'content-length',
                'content-encoding',
              ].contains(k.toLowerCase()),
            );
            response.headers[Headers.contentLengthHeader] = [
              bytes.length.toString(),
            ];
            final edited = bridge.beginBody(type);
            if (edited != null) {
              edited.add(bytes);
              await edited.close();
              responseData['body'] = edited.descriptor;
            }
          }
          responseData['status'] = response.statusCode;
          responseData['headers'] = _headers(response.headers);
          exchange['state'] = 'complete';
          emit();
        }
      }
      if (cancelFuture != null) {
        unawaited(
          cancelFuture.then((_) {
            if ([
              'pending',
              'held_request',
              'held_response',
            ].contains(exchange['state'])) {
              exchange['state'] = 'cancelled';
              emit();
            }
          }, onError: (Object _, StackTrace __) {}),
        );
      }
      emit();
      return response;
    } catch (error) {
      record.fail(error);
      rethrow;
    } finally {
      if (requestCapture != null && !requestCapture.isClosed) {
        await requestCapture.close(interrupted: true);
        request['body'] = requestCapture.descriptor;
        emit();
      }
    }
  }

  void _checkDecision(Json decision, RequestOptions options) {
    if (decision['action'] == 'drop' || decision['action'] == 'cancel') {
      throw DioException(
        requestOptions: options,
        type: DioExceptionType.cancel,
        error: decision['action'] == 'drop' ? 'inspector_drop' : 'cancelled',
      );
    }
  }

  Future<ResponseBody> _mock(
    Json decision,
    RequestOptions options,
    Json exchange,
    void Function() emit,
  ) async {
    final bytes = decision['bodyBase64'] == true
        ? base64Decode(decision['body'] as String? ?? '')
        : utf8.encode(decision['body'] as String? ?? '');
    if (bytes.length > owner.bridge.maxBodyBytes) {
      throw ArgumentError('Mock body exceeds capture limit');
    }
    final headers = _responseHeaders(
      decision['headers'] ??
          [
            {'name': 'content-type', 'value': 'application/json'},
          ],
    );
    headers.removeWhere(
      (key, _) =>
          ['content-length', 'content-encoding'].contains(key.toLowerCase()),
    );
    headers[Headers.contentLengthHeader] = [bytes.length.toString()];
    final body = owner.bridge.beginBody(headers['content-type']?.first ?? '');
    if (body != null) {
      body.add(bytes);
      await body.close();
    }
    final status = _status(decision);
    exchange['response'] = {
      'status': status,
      'headers': _headers(headers),
      'body': body?.descriptor,
    };
    exchange['state'] = 'complete';
    emit();
    return ResponseBody(
      Stream.value(Uint8List.fromList(bytes)),
      status,
      headers: headers,
    );
  }
}

int _status(Json decision) {
  final status = (decision['status'] as num? ?? 200).toInt();
  if (status < 100 || status > 599) {
    throw ArgumentError('HTTP status must be 100–599');
  }
  return status;
}

Stream<Uint8List> _tap(
  Stream<Uint8List> source,
  BodyCapture capture,
  void Function() finished, {
  void Function(Object)? onError,
}) async* {
  var ended = false;
  try {
    await for (final chunk in source) {
      capture.add(chunk);
      yield chunk;
    }
    ended = true;
  } catch (error) {
    onError?.call(error);
    rethrow;
  } finally {
    await capture.close(interrupted: !ended);
    finished();
  }
}

class _Buffered {
  _Buffered(this.stream, this.complete);
  final Stream<Uint8List>? stream;
  final bool complete;
}

Future<_Buffered> _buffer(
  Stream<Uint8List>? source,
  int limit,
  Future<void>? cancelled,
  RequestOptions options,
) async {
  if (source == null) return _Buffered(null, true);
  final iterator = StreamIterator(source);
  final prefix = <Uint8List>[];
  var size = 0;
  Future<bool> next() async {
    if (cancelled == null) return iterator.moveNext();
    return Future.any([
      iterator.moveNext(),
      cancelled.then<bool>(
        (_) => throw DioException(
          requestOptions: options,
          type: DioExceptionType.cancel,
        ),
      ),
    ]);
  }

  try {
    while (await next()) {
      prefix.add(iterator.current);
      size += iterator.current.length;
      if (size > limit) {
        Stream<Uint8List> resume() async* {
          try {
            for (final chunk in prefix) {
              yield chunk;
            }
            while (await iterator.moveNext()) {
              yield iterator.current;
            }
          } finally {
            await iterator.cancel();
          }
        }

        return _Buffered(resume(), false);
      }
    }
    await iterator.cancel();
    return _Buffered(Stream.fromIterable(prefix), true);
  } catch (_) {
    await iterator.cancel();
    rethrow;
  }
}
