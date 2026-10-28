library;

import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:shared_preferences/shared_preferences.dart';

class PreferencesInspector implements InspectorProvider {
  PreferencesInspector({
    required this.id,
    required this.preferences,
    required InspectorBridge bridge,
    String? name,
  }) : name = name ?? id {
    if (preferences is! SharedPreferences &&
        preferences is! SharedPreferencesAsync &&
        preferences is! SharedPreferencesWithCache) {
      throw ArgumentError(
        'Register a SharedPreferences, SharedPreferencesAsync, or SharedPreferencesWithCache instance',
      );
    }
    bridge.add(this);
  }
  @override
  final String id;
  @override
  final String name;
  final Object preferences;
  @override
  String get kind => 'preferences';

  Future<Map<String, Object?>> _read() async {
    final prefs = preferences;
    if (prefs is SharedPreferencesAsync) return prefs.getAll();
    if (prefs is SharedPreferences) {
      await prefs.reload();
      return {for (final key in prefs.getKeys()) key: prefs.get(key)};
    }
    final cache = prefs as SharedPreferencesWithCache;
    await cache.reloadCache();
    return {for (final key in cache.keys) key: cache.get(key)};
  }

  @override
  Future<dynamic> call(String method, Json params) async {
    if (method == 'preferences.list') {
      final values = await _read();
      return values.entries
          .map(
            (entry) => {
              'key': entry.key,
              'value': entry.value,
              'type': _type(entry.value),
            },
          )
          .toList()
        ..sort((a, b) => (a['key'] as String).compareTo(b['key'] as String));
    }
    final key = params['key'] as String;
    final dynamic prefs = preferences;
    if (method == 'preferences.remove') {
      await prefs.remove(key);
      return null;
    }
    if (method != 'preferences.set') {
      throw ArgumentError('Unsupported preferences operation');
    }
    final value = params['value'];
    switch (params['type']) {
      case 'String':
        if (value is! String) throw ArgumentError('Expected string');
        await prefs.setString(key, value);
      case 'int':
        if (value is! int) throw ArgumentError('Expected integer');
        await prefs.setInt(key, value);
      case 'double':
        if (value is! num) throw ArgumentError('Expected number');
        await prefs.setDouble(key, value.toDouble());
      case 'bool':
        if (value is! bool) throw ArgumentError('Expected boolean');
        await prefs.setBool(key, value);
      case 'List<String>':
        if (value is! List || value.any((v) => v is! String)) {
          throw ArgumentError('Expected string list');
        }
        await prefs.setStringList(key, value.cast<String>());
      default:
        throw ArgumentError('Unsupported preference type');
    }
    return null;
  }

  String _type(Object? value) => switch (value) {
    String() => 'String',
    bool() => 'bool',
    int() => 'int',
    double() => 'double',
    List() => 'List<String>',
    _ => 'null',
  };
  @override
  Future<void> dispose() async {}
}
