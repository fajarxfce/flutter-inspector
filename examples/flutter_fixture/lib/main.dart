import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:drift/drift.dart' as drift;
import 'package:drift/native.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:flutter_inspector_dio/flutter_inspector_dio.dart';
import 'package:flutter_inspector_drift/flutter_inspector_drift.dart';
import 'package:flutter_inspector_preferences/flutter_inspector_preferences.dart';
import 'package:shared_preferences/shared_preferences.dart';

late Fixture fixture;

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  fixture = await Fixture.create();
  runApp(FixtureApp(fixture: fixture));
}

class FixtureDatabase extends drift.GeneratedDatabase {
  FixtureDatabase() : super(NativeDatabase.memory());
  @override
  int get schemaVersion => 1;
  @override
  Iterable<drift.TableInfo<drift.Table, dynamic>> get allTables => [];
  @override
  List<drift.DatabaseSchemaEntity> get allSchemaEntities => [];
}

class AuditDatabase extends FixtureDatabase {}

class LateDatabase extends FixtureDatabase {}

class Fixture {
  Fixture(
    this.dio,
    this.database,
    this.audit,
    this.preferences,
    this.server,
    this.subscription,
    this.bridge,
    this.vmUri,
  );
  final Dio dio;
  final FixtureDatabase database;
  final AuditDatabase audit;
  final SharedPreferences preferences;
  final HttpServer server;
  final StreamSubscription<HttpRequest> subscription;
  final InspectorBridge? bridge;
  final String vmUri;
  LateDatabase? lateDatabase;
  Future<void> openLateDatabase() async {
    if (lateDatabase != null) return;
    final database = LateDatabase();
    lateDatabase = database;
    await database.customStatement(
      'CREATE TABLE late_notes(id INTEGER PRIMARY KEY, message TEXT NOT NULL)',
    );
    await database.customStatement(
      "INSERT INTO late_notes VALUES(1,'Opened after connect')",
    );
  }

  Future<void> closeLateDatabase() async {
    await lateDatabase?.close();
    lateDatabase = null;
  }

  static Future<Fixture> create() async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    // Ownership transfers to Fixture; Fixture.dispose cancels this subscription.
    // ignore: cancel_subscriptions
    final subscription = server.listen((request) async {
      try {
        final bytes = await request.fold<List<int>>([], (a, b) => a..addAll(b));
        if (request.uri.path == '/slow') {
          await Future<void>.delayed(const Duration(seconds: 4));
        }
        if (request.uri.path == '/binary') {
          request.response.headers.contentType = ContentType.binary;
          request.response.add(List.generate(256, (i) => i));
        } else {
          request.response.headers.contentType = ContentType.json;
          if (request.uri.path == '/error') request.response.statusCode = 422;
          request.response.write(
            jsonEncode({
              'method': request.method,
              'path': request.uri.path,
              'body': utf8.decode(bytes, allowMalformed: true),
              'headers': {
                for (final name in ['authorization', 'content-type', 'cookie'])
                  name: request.headers.value(name),
              },
              'user': {
                'id': 9007199254740993,
                'name': 'Fixture user',
                'roles': ['developer', 'tester'],
              },
              'items': List.generate(
                request.uri.path == '/large' ? 50000 : 4,
                (i) => {
                  'id': i,
                  'active': i.isEven,
                  'name': 'Item ' + i.toString(),
                },
              ),
            }),
          );
        }
        await request.response.close();
      } on HttpException {
        /* cancellation */
      } on SocketException {
        /* cancellation */
      }
    });
    final dio = Dio(
      BaseOptions(
        baseUrl: 'http://127.0.0.1:' + server.port.toString(),
        headers: {
          'authorization': 'Bearer fixture-original-token',
          'cookie': 'session=fixture-cookie',
        },
      ),
    );
    final preferences = await SharedPreferences.getInstance();
    await preferences.setString('auth.token', 'fixture-original-token');
    await preferences.setBool('feature.preview', true);
    await preferences.setStringList('recent.projects', [
      'storefront',
      'checkout',
    ]);
    final database = FixtureDatabase();
    await database.customStatement(
      'CREATE TABLE notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, completed INTEGER NOT NULL DEFAULT 0, payload BLOB)',
    );
    await database.customStatement(
      "INSERT INTO notes VALUES(1,'Inspect this row',0,X'00FF01'),(2,'Edit from the desktop',1,NULL)",
    );
    final audit = AuditDatabase();
    await audit.customStatement(
      'CREATE TABLE audit_log(id INTEGER PRIMARY KEY, message TEXT NOT NULL)',
    );
    await audit.customStatement(
      "INSERT INTO audit_log VALUES(1,'Audit database only')",
    );
    InspectorBridge? bridge;
    var uri = 'Available in debug builds';
    if (kDebugMode &&
        const bool.fromEnvironment('INSPECTOR_BRIDGE', defaultValue: true)) {
      bridge = await InspectorBridge.start(name: 'Flutter Inspector fixture');
      DioInspector(id: 'api', dio: dio, bridge: bridge, name: 'Fixture API');
      PreferencesInspector(
        id: 'preferences',
        preferences: preferences,
        bridge: bridge,
      );
      PreferencesInspector(
        id: 'preferences_async',
        preferences: SharedPreferencesAsync(),
        bridge: bridge,
      );
      final cache = await SharedPreferencesWithCache.create(
        cacheOptions: const SharedPreferencesWithCacheOptions(),
      );
      PreferencesInspector(
        id: 'preferences_cache',
        preferences: cache,
        bridge: bridge,
      );
      DriftInspector(
        id: 'database',
        database: database,
        bridge: bridge,
        name: 'Notes database',
      );
    }
    if (kDebugMode) {
      uri =
          (await developer.Service.getInfo()).serverUri?.toString() ??
          'Use the VM Service URI shown by flutter run';
    }
    return Fixture(
      dio,
      database,
      audit,
      preferences,
      server,
      subscription,
      bridge,
      uri,
    );
  }

  Future<void> dispose() async {
    await closeLateDatabase();
    await bridge?.dispose();
    dio.close(force: true);
    await database.close();
    await audit.close();
    await subscription.cancel();
    await server.close(force: true);
  }
}

class FixtureApp extends StatelessWidget {
  const FixtureApp({super.key, required this.fixture});
  final Fixture fixture;
  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'Inspector fixture',
    theme: ThemeData.dark(useMaterial3: true),
    home: FixtureScreen(fixture: fixture),
  );
}

class FixtureScreen extends StatefulWidget {
  const FixtureScreen({super.key, required this.fixture});
  final Fixture fixture;
  @override
  State<FixtureScreen> createState() => _FixtureScreenState();
}

class _FixtureScreenState extends State<FixtureScreen> {
  String result = 'Connect the Inspector, then generate traffic.';
  CancelToken? cancel;
  Future<void> send(
    String path, {
    bool post = false,
    bool multipart = false,
  }) async {
    setState(
      () =>
          result = 'Request running. Check the intercept queue if it is held.',
    );
    cancel = CancelToken();
    try {
      final response = await widget.fixture.dio.request<dynamic>(
        path,
        cancelToken: cancel,
        data: multipart
            ? FormData.fromMap({
                'file': MultipartFile.fromBytes([
                  0,
                  255,
                  1,
                  2,
                ], filename: 'fixture.bin'),
                'label': 'sample',
              })
            : post
            ? {'message': 'Hello from Flutter', 'count': 3}
            : null,
        options: Options(
          method: post || multipart ? 'POST' : 'GET',
          responseType: path == '/binary'
              ? ResponseType.bytes
              : ResponseType.json,
        ),
      );
      if (mounted) {
        setState(
          () => result =
              'Status ' +
              response.statusCode.toString() +
              ' · ' +
              response.data.toString().substring(
                0,
                response.data.toString().length.clamp(0, 1200),
              ),
        );
      }
    } catch (error) {
      if (mounted) setState(() => result = error.toString());
    }
  }

  @override
  void dispose() {
    unawaited(widget.fixture.dispose());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Flutter Inspector · fixture')),
    body: SingleChildScrollView(
      padding: const EdgeInsets.all(32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('VM Service URI', style: TextStyle(fontSize: 20)),
          const SizedBox(height: 8),
          SelectableText(widget.fixture.vmUri),
          const SizedBox(height: 24),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: [
              FilledButton(
                onPressed: () => send('/json'),
                child: const Text('GET JSON'),
              ),
              FilledButton(
                onPressed: () => send('/echo', post: true),
                child: const Text('POST JSON'),
              ),
              OutlinedButton(
                onPressed: () => send('/multipart', multipart: true),
                child: const Text('Multipart'),
              ),
              OutlinedButton(
                onPressed: () => send('/large'),
                child: const Text('Large JSON'),
              ),
              OutlinedButton(
                onPressed: () => send('/binary'),
                child: const Text('Binary'),
              ),
              OutlinedButton(
                onPressed: () => send('/error'),
                child: const Text('HTTP 422'),
              ),
              OutlinedButton(
                onPressed: () => send('/slow'),
                child: const Text('Slow request'),
              ),
              OutlinedButton(
                onPressed: () => cancel?.cancel('Cancelled in fixture'),
                child: const Text('Cancel'),
              ),
              OutlinedButton(
                onPressed: () async {
                  await widget.fixture.preferences.reload();
                  final rows = await widget.fixture.database
                      .customSelect('SELECT * FROM notes')
                      .get();
                  if (mounted) {
                    setState(
                      () => result =
                          'Preferences: ' +
                          widget.fixture.preferences
                              .getKeys()
                              .map(
                                (k) =>
                                    k +
                                    '=' +
                                    widget.fixture.preferences
                                        .get(k)
                                        .toString(),
                              )
                              .join(', ') +
                          '\nDatabase: ' +
                          rows.map((r) => r.data).toString(),
                    );
                  }
                },
                child: const Text('Read edited storage'),
              ),
              OutlinedButton(
                onPressed: widget.fixture.openLateDatabase,
                child: const Text('Open late database'),
              ),
              OutlinedButton(
                onPressed: widget.fixture.closeLateDatabase,
                child: const Text('Close late database'),
              ),
            ],
          ),
          const SizedBox(height: 28),
          SelectableText(result),
        ],
      ),
    ),
  );
}
