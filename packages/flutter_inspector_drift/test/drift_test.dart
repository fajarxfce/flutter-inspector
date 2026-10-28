import 'dart:convert';
import 'package:drift/drift.dart';
import 'package:drift/native.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:flutter_inspector_drift/flutter_inspector_drift.dart';
import 'package:test/test.dart';

class TestDatabase extends GeneratedDatabase {
  TestDatabase() : super(NativeDatabase.memory());
  @override
  int get schemaVersion => 1;
  @override
  Iterable<TableInfo<Table, dynamic>> get allTables => [];
  @override
  List<DatabaseSchemaEntity> get allSchemaEntities => [];
}

void main() {
  late InspectorBridge bridge;
  late TestDatabase db;
  late DriftInspector inspector;
  setUp(() async {
    bridge = await InspectorBridge.start();
    db = TestDatabase();
    await db.customStatement(
      'CREATE TABLE records(tenant TEXT, id INTEGER, name TEXT, payload BLOB, PRIMARY KEY(tenant,id))',
    );
    inspector = DriftInspector(id: 'db', database: db, bridge: bridge);
  });
  tearDown(() async {
    await bridge.dispose();
    await db.close();
  });
  test(
    'composite-key CRUD, BLOB, quoted values, filtering and sorting',
    () async {
      await inspector.call('drift.insert', {
        'table': 'records',
        'values': {
          'tenant': 'one',
          'id': 1,
          'name': "O'Reilly",
          'payload': {
            'base64': base64Encode([0, 255, 1]),
          },
        },
      });
      final result = await inspector.call('drift.rows', {
        'table': 'records',
        'filter': "O'Reilly",
        'order': 'id',
        'descending': true,
      });
      expect(result['total'], 1);
      expect(result['rows'][0]['payload'], {'base64': 'AP8B'});
      await expectLater(
        inspector.call('drift.update', {
          'table': 'records',
          'identity': {'id': 1},
          'values': {'name': 'wrong'},
        }),
        throwsArgumentError,
      );
      await inspector.call('drift.update', {
        'table': 'records',
        'identity': {'tenant': 'one', 'id': 1},
        'values': {'name': 'changed'},
      });
      expect(
        (await inspector.call('drift.query', {
          'sql': 'SELECT name FROM records;',
        }))[0]['name'],
        'changed',
      );
      await inspector.call('drift.delete', {
        'table': 'records',
        'identity': {'tenant': 'one', 'id': 1},
      });
      expect(
        (await inspector.call('drift.rows', {'table': 'records'}))['total'],
        0,
      );
    },
  );
  test(
    'one statement, identifier validation, read-only view and query cap',
    () async {
      expect(singleStatement("SELECT ';' AS x; -- comment"), "SELECT ';' AS x");
      expect(
        () => singleStatement('SELECT 1; DELETE FROM records'),
        throwsArgumentError,
      );
      await expectLater(
        inspector.call('drift.rows', {
          'table': 'records";DROP TABLE records;--',
        }),
        throwsArgumentError,
      );
      await inspector.call('drift.execute', {
        'sql': 'CREATE VIEW summary AS SELECT * FROM records',
      });
      await expectLater(
        inspector.call('drift.insert', {
          'table': 'summary',
          'values': {'id': 1},
        }),
        throwsStateError,
      );
      final rows = await inspector.call('drift.query', {
        'sql':
            'WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1500) SELECT x FROM n',
      });
      expect(rows, hasLength(1000));
      await expectLater(
        inspector.call('drift.query', {'sql': 'DELETE FROM records'}),
        throwsArgumentError,
      );
    },
  );
  test(
    'grid batches are atomic, detect stale edits and delete selected rows',
    () async {
      await inspector.call('drift.apply', {
        'table': 'records',
        'changes': [
          {
            'kind': 'insert',
            'values': {'tenant': 'one', 'id': 1, 'name': 'before'},
          },
          {
            'kind': 'insert',
            'values': {'tenant': 'one', 'id': 2, 'name': 'second'},
          },
        ],
      });
      await expectLater(
        inspector.call('drift.apply', {
          'table': 'records',
          'changes': [
            {
              'kind': 'update',
              'identity': {'tenant': 'one', 'id': 1},
              'values': {'name': 'must roll back'},
            },
            {
              'kind': 'insert',
              'values': {'tenant': 'one', 'id': 2},
            },
          ],
        }),
        throwsA(anything),
      );
      expect(
        (await inspector.call('drift.query', {
          'sql': 'SELECT name FROM records WHERE id=1',
        }))[0]['name'],
        'before',
      );
      await expectLater(
        inspector.call('drift.apply', {
          'table': 'records',
          'changes': [
            {
              'kind': 'update',
              'identity': {'tenant': 'one', 'id': 1},
              'original': {'name': 'stale'},
              'values': {'name': 'lost update'},
            },
          ],
        }),
        throwsStateError,
      );
      final result = await inspector.call('drift.apply', {
        'table': 'records',
        'changes': [
          {
            'kind': 'update',
            'identity': {'tenant': 'one', 'id': 1},
            'original': {'name': 'before'},
            'values': {
              'name': 'after',
              'payload': {'base64': 'AP8='},
            },
          },
          {
            'kind': 'delete',
            'identity': {'tenant': 'one', 'id': 2},
          },
        ],
      });
      expect(result['changed'], 2);
      final rows = await inspector.call('drift.rows', {'table': 'records'});
      expect(rows['total'], 1);
      expect(rows['rows'][0]['name'], 'after');
      expect(rows['rows'][0]['payload'], {'base64': 'AP8='});
    },
  );
  test(
    'new rows use database defaults; empty keeps schema and respects foreign keys',
    () async {
      await db.customStatement('PRAGMA foreign_keys=ON');
      await db.customStatement(
        "CREATE TABLE defaults_test(id INTEGER PRIMARY KEY, title TEXT NOT NULL DEFAULT 'new', computed TEXT GENERATED ALWAYS AS (title || '!') STORED)",
      );
      await inspector.call('drift.apply', {
        'table': 'defaults_test',
        'changes': [
          {'kind': 'insert', 'values': {}},
        ],
      });
      var rows = await inspector.call('drift.rows', {'table': 'defaults_test'});
      expect(rows['rows'][0]['title'], 'new');
      expect(rows['rows'][0]['computed'], 'new!');
      await expectLater(
        inspector.call('drift.apply', {
          'table': 'defaults_test',
          'changes': [
            {
              'kind': 'update',
              'identity': {'id': 1},
              'values': {'computed': 'forbidden'},
            },
          ],
        }),
        throwsArgumentError,
      );
      await expectLater(
        inspector.call('drift.empty', {
          'table': 'defaults_test',
          'confirm': 'wrong',
        }),
        throwsArgumentError,
      );
      await db.customStatement(
        'CREATE TABLE child(parent INTEGER REFERENCES defaults_test(id))',
      );
      await db.customStatement('INSERT INTO child VALUES(1)');
      await expectLater(
        inspector.call('drift.empty', {
          'table': 'defaults_test',
          'confirm': 'defaults_test',
        }),
        throwsA(anything),
      );
      expect(
        (await inspector.call('drift.rows', {
          'table': 'defaults_test',
        }))['total'],
        1,
      );
      await db.customStatement('DELETE FROM child');
      expect(
        (await inspector.call('drift.empty', {
          'table': 'defaults_test',
          'confirm': 'defaults_test',
        }))['changed'],
        1,
      );
      rows = await inspector.call('drift.rows', {'table': 'defaults_test'});
      expect(rows['total'], 0);
      expect(rows['columns'], hasLength(3));
    },
  );
}
