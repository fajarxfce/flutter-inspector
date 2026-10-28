library;

import 'dart:convert';
import 'package:drift/drift.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';

String _identifier(String name) => '"' + name.replaceAll('"', '""') + '"';
Object? _encode(Object? value) =>
    value is Uint8List ? {'base64': base64Encode(value)} : value;
Object? _decode(Object? value) =>
    value is Map && value.length == 1 && value['base64'] is String
    ? base64Decode(value['base64'] as String)
    : value;

class DriftInspector implements InspectorProvider {
  DriftInspector({
    required this.id,
    required this.database,
    required InspectorBridge bridge,
    String? name,
  }) : name = name ?? id {
    bridge.add(this);
  }
  @override
  final String id;
  @override
  final String name;
  final GeneratedDatabase database;
  @override
  String get kind => 'drift';

  Future<List<Json>> _select(
    String sql, [
    List<Object?> args = const [],
  ]) async {
    final result = await database
        .customSelect(
          sql,
          variables: args.map((v) => Variable(_decode(v))).toList(),
        )
        .get();
    return result
        .map(
          (row) => row.data.map((key, value) => MapEntry(key, _encode(value))),
        )
        .toList();
  }

  Future<List<Json>> _columns(String table) async {
    final exists = await _select(
      "SELECT type FROM sqlite_master WHERE name=? AND type IN ('table','view')",
      [table],
    );
    if (exists.isEmpty) throw ArgumentError('Unknown table or view');
    return _select('PRAGMA table_xinfo(' + _identifier(table) + ')');
  }

  Future<Json> _apply(
    String table,
    List<Json> columns,
    Json params, {
    bool empty = false,
  }) async {
    final changes = empty ? <dynamic>[] : params['changes'] as List;
    if (!empty && (changes.isEmpty || changes.length > 200)) {
      throw ArgumentError('Save between 1 and 200 rows at a time');
    }
    if (utf8.encode(jsonEncode(changes)).length > 2 * 1024 * 1024) {
      throw ArgumentError('Changes exceed 2 MiB');
    }
    final names = columns
        .where((c) => (c['hidden'] ?? 0) == 0)
        .map((c) => c['name'] as String)
        .toSet();
    final keys = columns
        .where((c) => (c['pk'] as num? ?? 0) > 0)
        .map((c) => c['name'] as String)
        .toList();
    final target = _identifier(table);
    var affected = 0;
    await database.transaction(() async {
      if (empty) {
        await database.customStatement('DELETE FROM ' + target);
        affected = (await _select('SELECT changes() AS n')).first['n'] as int;
      }
      for (final raw in changes) {
        final change = (raw as Map).cast<String, dynamic>();
        final values =
            (change['values'] as Map?)?.cast<String, dynamic>() ??
            <String, dynamic>{};
        final original =
            (change['original'] as Map?)?.cast<String, dynamic>() ??
            <String, dynamic>{};
        if ([...values.keys, ...original.keys].any((k) => !names.contains(k))) {
          throw ArgumentError('Unknown or generated column');
        }
        final kind = change['kind'];
        var args = values.values.map(_decode).toList();
        late String sql;
        if (kind == 'insert') {
          sql = values.isEmpty
              ? 'INSERT INTO ' + target + ' DEFAULT VALUES'
              : 'INSERT INTO ' +
                    target +
                    ' (' +
                    values.keys.map(_identifier).join(',') +
                    ') VALUES (' +
                    List.filled(values.length, '?').join(',') +
                    ')';
        } else {
          final identity =
              (change['identity'] as Map?)?.cast<String, dynamic>() ??
              <String, dynamic>{};
          if (keys.isEmpty || keys.any((k) => identity[k] == null)) {
            throw ArgumentError('A complete non-null primary key is required');
          }
          final conditions = [
            for (final k in keys) MapEntry(k, identity[k]),
            ...original.entries,
          ];
          final where = conditions
              .map((e) => _identifier(e.key) + ' IS ?')
              .join(' AND ');
          final conditionArgs = conditions
              .map((e) => _decode(e.value))
              .toList();
          if (kind == 'update') {
            if (values.isEmpty) throw ArgumentError('Provide values to update');
            sql =
                'UPDATE ' +
                target +
                ' SET ' +
                values.keys.map((k) => _identifier(k) + '=?').join(',') +
                ' WHERE ' +
                where;
            args.addAll(conditionArgs);
          } else if (kind == 'delete') {
            sql = 'DELETE FROM ' + target + ' WHERE ' + where;
            args = conditionArgs;
          } else {
            throw ArgumentError('Unsupported change kind');
          }
        }
        await database.customStatement(sql, args);
        final count =
            (await _select('SELECT changes() AS n')).first['n'] as int;
        if (kind != 'insert' && count != 1) {
          throw StateError(
            'Row changed or disappeared. Refresh before trying again.',
          );
        }
        affected += count;
      }
      database.notifyUpdates({TableUpdate(table)});
    });
    return {'changed': affected};
  }

  @override
  Future<dynamic> call(String method, Json params) async {
    if (method == 'drift.tables') {
      return _select(
        "SELECT name,type,sql FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
      );
    }
    if (method == 'drift.query') {
      final sql = singleStatement(params['sql'] as String);
      if (!RegExp(r'^\s*(SELECT|WITH)\b', caseSensitive: false).hasMatch(sql)) {
        throw ArgumentError(
          'Query mode accepts SELECT/WITH; use Execute for other statements',
        );
      }
      return _select('SELECT * FROM (' + sql + ') LIMIT 1000');
    }
    if (method == 'drift.execute') {
      final sql = singleStatement(params['sql'] as String);
      await database.customStatement(sql);
      database.notifyUpdates(
        database.allTables.map((t) => TableUpdate(t.actualTableName)).toSet(),
      );
      return {'executed': true};
    }
    final table = params['table'] as String;
    final columns = await _columns(table);
    if (method == 'drift.columns') return columns;
    final names = columns.map((c) => c['name'] as String).toSet();
    if (method == 'drift.rows') {
      final offset = (params['offset'] as num? ?? 0).toInt();
      if (offset < 0) throw ArgumentError('Offset must be positive');
      final filter = params['filter'] as String? ?? '';
      final order = params['order'] as String? ?? '';
      if (order.isNotEmpty && !names.contains(order)) {
        throw ArgumentError('Unknown sort column');
      }
      final args = <Object?>[];
      var where = '';
      if (filter.isNotEmpty) {
        where =
            ' WHERE ' +
            names
                .map((n) {
                  args.add('%' + filter + '%');
                  return 'CAST(' + _identifier(n) + ' AS TEXT) LIKE ?';
                })
                .join(' OR ');
      }
      final count = await _select(
        'SELECT COUNT(*) AS total FROM ' + _identifier(table) + where,
        args,
      );
      final sorting = order.isEmpty
          ? ''
          : ' ORDER BY ' +
                _identifier(order) +
                (params['descending'] == true ? ' DESC' : ' ASC');
      final rows = await _select(
        'SELECT * FROM ' +
            _identifier(table) +
            where +
            sorting +
            ' LIMIT 200 OFFSET ?',
        [...args, offset],
      );
      return {'columns': columns, 'rows': rows, 'total': count.first['total']};
    }
    final tableType = await _select(
      'SELECT type FROM sqlite_master WHERE name=?',
      [table],
    );
    if (tableType.first['type'] != 'table') {
      throw StateError('Views are read-only');
    }
    if (method == 'drift.apply') return _apply(table, columns, params);
    if (method == 'drift.empty') {
      if (params['confirm'] != table) {
        throw ArgumentError('Confirm the table name before emptying it');
      }
      return _apply(table, columns, {}, empty: true);
    }
    final values =
        (params['values'] as Map?)?.cast<String, dynamic>() ??
        <String, dynamic>{};
    if (values.keys.any((key) => !names.contains(key))) {
      throw ArgumentError('Unknown column');
    }
    if (method == 'drift.insert') {
      if (values.isEmpty) throw ArgumentError('Provide values to insert');
      await database.customStatement(
        'INSERT INTO ' +
            _identifier(table) +
            ' (' +
            values.keys.map(_identifier).join(',') +
            ') VALUES (' +
            List.filled(values.length, '?').join(',') +
            ')',
        values.values.map(_decode).toList(),
      );
    } else {
      final keys = columns
          .where((c) => (c['pk'] as num? ?? 0) > 0)
          .map((c) => c['name'] as String)
          .toList();
      final identity =
          (params['identity'] as Map?)?.cast<String, dynamic>() ??
          <String, dynamic>{};
      if (keys.isEmpty || keys.any((k) => !identity.containsKey(k))) {
        throw ArgumentError('A complete primary key is required');
      }
      final where = keys.map((k) => _identifier(k) + ' IS ?').join(' AND ');
      final keyValues = keys.map((k) => _decode(identity[k])).toList();
      if (method == 'drift.update') {
        if (values.isEmpty) throw ArgumentError('Provide values to update');
        await database.customStatement(
          'UPDATE ' +
              _identifier(table) +
              ' SET ' +
              values.keys.map((k) => _identifier(k) + '=?').join(',') +
              ' WHERE ' +
              where,
          [...values.values.map(_decode), ...keyValues],
        );
      } else if (method == 'drift.delete') {
        await database.customStatement(
          'DELETE FROM ' + _identifier(table) + ' WHERE ' + where,
          keyValues,
        );
      } else {
        throw ArgumentError('Unsupported Drift operation');
      }
    }
    database.notifyUpdates({TableUpdate(table)});
    return {'changed': true};
  }

  @override
  Future<void> dispose() async {}
}

/// Accepts one SQLite statement, including quoted semicolons and comments.
String singleStatement(String input) {
  if (input.length > 65536) throw ArgumentError('SQL exceeds 64 KiB');
  var quote = '';
  var line = false;
  var block = false;
  var ended = false;
  final output = StringBuffer();
  for (var i = 0; i < input.length; i++) {
    final c = input[i];
    final next = i + 1 < input.length ? input[i + 1] : '';
    if (line) {
      if (c == '\n') {
        line = false;
        output.write(' ');
      }
      continue;
    }
    if (block) {
      if (c == '*' && next == '/') {
        block = false;
        i++;
        output.write(' ');
      }
      continue;
    }
    if (quote.isNotEmpty) {
      output.write(c);
      if (c == quote) {
        if (next == quote) {
          output.write(next);
          i++;
        } else {
          quote = '';
        }
      }
      continue;
    }
    if (c == '-' && next == '-') {
      line = true;
      i++;
      continue;
    }
    if (c == '/' && next == '*') {
      block = true;
      i++;
      continue;
    }
    if (c == ';') {
      if (ended) throw ArgumentError('Only one SQL statement is allowed');
      ended = true;
      continue;
    }
    if (ended) {
      if (c.trim().isNotEmpty) {
        throw ArgumentError('Only one SQL statement is allowed');
      }
      continue;
    }
    if (c == "'" || c == '"') {
      quote = c;
    }
    output.write(c);
  }
  if (quote.isNotEmpty || block) {
    throw ArgumentError('Unterminated SQL quote or comment');
  }
  final sql = output.toString().trim();
  if (sql.isEmpty) throw ArgumentError('SQL is empty');
  return sql;
}
