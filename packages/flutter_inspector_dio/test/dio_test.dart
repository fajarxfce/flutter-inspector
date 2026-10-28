import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:dio/dio.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:flutter_inspector_dio/flutter_inspector_dio.dart';
import 'package:test/test.dart';

void main() {
  late HttpServer server;
  late StreamSubscription<HttpRequest> subscription;
  late Dio dio;
  late InspectorBridge bridge;
  late DioInspector inspector;
  var hits = 0;
  Future<dynamic> call(String method, [Json params = const {}]) => bridge
      .dispatch({'controller': 'test', 'method': method, 'params': params});
  Future<List<dynamic>> events() async =>
      (await call('events'))['events'] as List;
  Future<Json> latest() async => ((await events()).last as Map)['data'] as Json;
  Future<Json> held() async {
    for (var i = 0; i < 300; i++) {
      final pending = (await call('events'))['pending'] as List;
      if (pending.isNotEmpty) {
        return (pending.first as Map).cast<String, dynamic>();
      }
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    throw StateError('Request did not enter intercept queue');
  }

  Future<void> rules({
    bool request = true,
    bool response = false,
    int seconds = 120,
  }) async {
    await call('intercept.rules', {
      'enabled': true,
      'host': '127.0.0.1',
      'path': '',
      'method': '',
      'request': request,
      'response': response,
      'holdSeconds': seconds,
    });
  }

  Future<List<int>> body(Json descriptor) async {
    final result = await call('body.read', {
      'id': descriptor['id'],
      'offset': 0,
    });
    return base64Decode(result['data'] as String);
  }

  setUp(() async {
    hits = 0;
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    subscription = server.listen((request) async {
      hits++;
      try {
        final bytes = await request.fold<List<int>>([], (a, b) => a..addAll(b));
        if (request.uri.path == '/slow') {
          await Future<void>.delayed(const Duration(milliseconds: 500));
        }
        if (request.uri.path == '/bytes') {
          request.response.add([0, 255, 1, 128, 2]);
          await request.response.close();
          return;
        }
        request.response.headers.contentType = ContentType.json;
        if (request.uri.path == '/invalid') {
          request.response.write('{broken');
          await request.response.close();
          return;
        }
        if (request.uri.path == '/status') request.response.statusCode = 422;
        request.response.write(
          jsonEncode({
            'method': request.method,
            'path': request.uri.path,
            'body': utf8.decode(bytes, allowMalformed: true),
            'token': request.headers.value('authorization'),
            'large': request.uri.path == '/large' ? 'x' * 2048 : '',
          }),
        );
        await request.response.close();
      } on HttpException {
        /* cancelled request */
      } on SocketException {
        /* closed peer */
      }
    });
    bridge = await InspectorBridge.start(
      maxBodyBytes: 1024,
      maxSpoolBytes: 8192,
    );
    dio = Dio(
      BaseOptions(baseUrl: 'http://127.0.0.1:' + server.port.toString()),
    );
    inspector = DioInspector(id: 'api', dio: dio, bridge: bridge);
    await call('hello');
  });
  tearDown(() async {
    dio.close(force: true);
    await bridge.dispose();
    await subscription.cancel();
    await server.close(force: true);
  });
  test(
    'captures original credentials, transformed request bytes, and JSON response',
    () async {
      final response = await dio.post<dynamic>(
        '/echo',
        data: {'id': 9007199254740993, 'name': 'original'},
        options: Options(headers: {'Authorization': 'Bearer original-secret'}),
      );
      expect(response.data['token'], 'Bearer original-secret');
      final exchange = await latest();
      expect(exchange['state'], 'complete');
      expect(
        utf8.decode(await body(exchange['request']['body'] as Json)),
        jsonEncode({'id': 9007199254740993, 'name': 'original'}),
      );
      expect(
        jsonDecode(
          utf8.decode(await body(exchange['response']['body'] as Json)),
        )['token'],
        'Bearer original-secret',
      );
    },
  );
  test('edits request before sending and fixes content length', () async {
    await rules();
    final response = dio.post<dynamic>(
      '/echo',
      data: 'old',
      options: Options(contentType: 'text/plain'),
    );
    final pending = await held();
    expect(hits, 0);
    await call('network.decide', {
      'id': pending['id'],
      'action': 'forward',
      'method': 'PUT',
      'body': 'new and longer',
      'headers': [
        {'name': 'content-type', 'value': 'text/plain'},
        {'name': 'content-length', 'value': '999'},
      ],
    });
    final result = await response;
    expect(result.data['method'], 'PUT');
    expect(result.data['body'], 'new and longer');
    expect(hits, 1);
  });
  test('edits response before Dio JSON transformation', () async {
    await rules(request: false, response: true);
    final response = dio.get<dynamic>('/echo');
    final pending = await held();
    expect(pending['direction'], 'response');
    await call('network.decide', {
      'id': pending['id'],
      'action': 'forward',
      'status': 201,
      'body': '{"edited":true}',
    });
    final result = await response;
    expect(result.statusCode, 201);
    expect(result.data, {'edited': true});
  });
  test('mock avoids network and drop completes as cancellation', () async {
    await rules();
    final response = dio.get<dynamic>('/echo');
    var pending = await held();
    await call('network.decide', {
      'id': pending['id'],
      'action': 'mock',
      'status': 202,
      'body': '{"mocked":true}',
    });
    expect((await response).data, {'mocked': true});
    expect(hits, 0);
    final dropped = expectLater(
      dio.get<dynamic>('/echo'),
      throwsA(
        isA<DioException>().having(
          (e) => e.type,
          'type',
          DioExceptionType.cancel,
        ),
      ),
    );
    pending = await held();
    await call('network.decide', {'id': pending['id'], 'action': 'drop'});
    await dropped;
    expect((await latest())['state'], 'dropped');
  });
  test('cancel while held and detach forward original', () async {
    await rules();
    final token = CancelToken();
    final cancelled = expectLater(
      dio.get<dynamic>('/echo', cancelToken: token),
      throwsA(isA<DioException>()),
    );
    await held();
    token.cancel();
    await cancelled;
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect((await call('events'))['pending'], isEmpty);
    final response = dio.post<dynamic>(
      '/echo',
      data: 'original',
      options: Options(contentType: 'text/plain'),
    );
    await held();
    await call('detach');
    expect((await response).data['body'], 'original');
  });
  test(
    'bounded capture never truncates the response delivered to the app',
    () async {
      final response = await dio.get<dynamic>('/large');
      expect(response.data['large'], hasLength(2048));
      final captured = (await latest())['response']['body'] as Json;
      expect(captured['size'], 1024);
      expect(captured['complete'], false);
    },
  );
  test('binary and multipart request bodies remain replayable', () async {
    final response = await dio.get<List<int>>(
      '/bytes',
      options: Options(responseType: ResponseType.bytes),
    );
    expect(response.data, [0, 255, 1, 128, 2]);
    expect(
      await body((await latest())['response']['body'] as Json),
      response.data,
    );
    await dio.post<dynamic>(
      '/echo',
      data: FormData.fromMap({
        'name': 'fixture',
        'file': MultipartFile.fromBytes([1, 2, 3], filename: 'sample.bin'),
      }),
    );
    final captured = (await latest())['request']['body'] as Json;
    expect(captured['complete'], true);
    expect(
      utf8.decode(await body(captured), allowMalformed: true),
      contains('sample.bin'),
    );
  });
  test('app replay runs app interceptors and skips intercept rules', () async {
    dio.interceptors.add(
      InterceptorsWrapper(
        onRequest: (options, handler) {
          options.headers['authorization'] = 'Bearer from-app';
          handler.next(options);
        },
      ),
    );
    await rules(request: true, response: true);
    final result = await inspector.call('replay', {
      'id': 'run-one',
      'draftId': 'tab-one',
      'url': dio.options.baseUrl + '/echo',
      'method': 'POST',
      'headers': [],
      'body': 'replayed',
    });
    expect(result['status'], 200);
    final exchange = await latest();
    expect(exchange['source'], 'repeater_app');
    expect(exchange['id'], 'run-one');
    expect(exchange['repeaterId'], 'tab-one');
    expect(result['exchange']['id'], 'run-one');
    expect(
      result['exchange']['response']['body'],
      exchange['response']['body'],
    );
    expect(
      jsonDecode(
        utf8.decode(await body(exchange['response']['body'] as Json)),
      )['token'],
      'Bearer from-app',
    );
    expect((await call('events'))['pending'], isEmpty);
  });
  test('replay cancellation reaches registered Dio CancelToken', () async {
    final result = expectLater(
      inspector.call('replay', {
        'id': 'draft',
        'url': dio.options.baseUrl + '/slow',
        'method': 'GET',
        'headers': [],
      }),
      throwsA(isA<DioException>()),
    );
    await Future<void>.delayed(const Duration(milliseconds: 30));
    await inspector.call('replay.cancel', {'id': 'draft'});
    await result;
  });
  test(
    'errors after adapter return are recorded and original adapter restored',
    () async {
      await expectLater(
        dio.get<dynamic>('/invalid'),
        throwsA(isA<DioException>()),
      );
      expect((await latest())['state'], 'error');
      await expectLater(
        dio.get<dynamic>('/status'),
        throwsA(isA<DioException>()),
      );
      expect((await latest())['response']['status'], 422);
      await inspector.dispose();
      expect((await dio.get<dynamic>('/echo')).statusCode, 200);
    },
  );
  test('streaming request and response are consumed only by the app', () async {
    await rules(request: true, response: true);
    final response = await dio.post<ResponseBody>(
      '/echo',
      data: Stream.value(Uint8List.fromList(utf8.encode('stream'))),
      options: Options(
        responseType: ResponseType.stream,
        contentType: 'text/plain',
        headers: {'content-length': 6},
      ),
    );
    expect((await call('events'))['pending'], isEmpty);
    final bytes = await response.data!.stream.fold<List<int>>(
      [],
      (a, b) => a..addAll(b),
    );
    expect(jsonDecode(utf8.decode(bytes))['body'], 'stream');
  });
  test(
    'unconsumed uploads and error responses do not leak capture slots',
    () async {
      final failed = Dio()..httpClientAdapter = _RejectingAdapter();
      DioInspector(id: 'failing', dio: failed, bridge: bridge);
      try {
        for (var i = 0; i < 70; i++) {
          await expectLater(
            failed.post<dynamic>('http://127.0.0.1/failure', data: 'unsent'),
            throwsA(isA<DioException>()),
          );
          await expectLater(
            dio.get<dynamic>(
              '/status',
              options: Options(receiveDataWhenStatusError: false),
            ),
            throwsA(isA<DioException>()),
          );
        }
        final capture = bridge.beginBody('text/plain');
        expect(capture, isNotNull);
        await capture!.close();
      } finally {
        failed.close(force: true);
      }
    },
  );
}

class _RejectingAdapter implements HttpClientAdapter {
  @override
  Future<ResponseBody> fetch(
    RequestOptions options,
    Stream<Uint8List>? requestStream,
    Future<void>? cancelFuture,
  ) async {
    throw const SocketException('Fixture connection failure before upload');
  }

  @override
  void close({bool force = false}) {}
}
