import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:flutter_inspector_dio/flutter_inspector_dio.dart';

/// A real VM service + HTTP server used by the Rust protocol integration test.
Future<void> main() async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  final subscription = server.listen((request) async {
    final body = await utf8.decoder.bind(request).join();
    request.response.headers.contentType = ContentType.json;
    request.response.write(
      jsonEncode({
        'body': body,
        'method': request.method,
        'token': request.headers.value('authorization'),
        'number': 9007199254740993,
      }),
    );
    await request.response.close();
  });
  final bridge = await InspectorBridge.start(name: 'Protocol fixture');
  final dio = Dio(
    BaseOptions(baseUrl: 'http://127.0.0.1:' + server.port.toString()),
  );
  DioInspector(id: 'api', dio: dio, bridge: bridge);
  bridge.add(_Fixture(dio));
  try {
    await Completer<void>().future;
  } finally {
    await subscription.cancel();
    await server.close(force: true);
    dio.close(force: true);
    await bridge.dispose();
  }
}

class _Fixture implements InspectorProvider {
  _Fixture(this.dio);
  final Dio dio;
  @override
  String get id => 'fixture';
  @override
  String get name => 'HTTP fixture';
  @override
  String get kind => 'fixture';
  @override
  Future<dynamic> call(String method, Json params) async {
    if (method == 'fixture.info') return {'url': dio.options.baseUrl};
    if (method != 'fixture.request') {
      throw ArgumentError('Unknown fixture method');
    }
    final response = await dio.post<dynamic>(
      '/echo',
      data: params['body'] ?? 'original',
      options: Options(
        contentType: 'text/plain',
        headers: {'authorization': 'Bearer original-secret'},
      ),
    );
    return {'status': response.statusCode, 'data': response.data};
  }

  @override
  Future<void> dispose() async {}
}
