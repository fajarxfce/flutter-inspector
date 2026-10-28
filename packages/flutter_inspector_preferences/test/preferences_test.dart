import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';
import 'package:flutter_inspector_preferences/flutter_inspector_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late InspectorBridge bridge;
  late PreferencesInspector inspector;
  late SharedPreferences preferences;
  setUp(() async {
    SharedPreferences.setMockInitialValues({'token': 'original', 'count': 1});
    preferences = await SharedPreferences.getInstance();
    bridge = await InspectorBridge.start();
    inspector = PreferencesInspector(
      id: 'prefs',
      preferences: preferences,
      bridge: bridge,
    );
  });
  tearDown(() async {
    await bridge.dispose();
  });
  test(
    'reads and edits the registered instance with original typed values',
    () async {
      final rows = await inspector.call('preferences.list', {});
      expect(rows, [
        {'key': 'count', 'value': 1, 'type': 'int'},
        {'key': 'token', 'value': 'original', 'type': 'String'},
      ]);
      for (final entry in <String, dynamic>{
        'String': 'secret',
        'bool': true,
        'int': 123,
        'double': 1.25,
        'List<String>': ['a', 'b'],
      }.entries) {
        await inspector.call('preferences.set', {
          'key': 'sample',
          'type': entry.key,
          'value': entry.value,
        });
        expect(preferences.get('sample'), entry.value);
      }
      await inspector.call('preferences.remove', {'key': 'token'});
      expect(preferences.containsKey('token'), false);
      await expectLater(
        inspector.call('preferences.set', {
          'key': 'bad',
          'type': 'int',
          'value': 'wrong',
        }),
        throwsArgumentError,
      );
      expect(
        () => PreferencesInspector(
          id: 'bad',
          preferences: Object(),
          bridge: bridge,
        ),
        throwsArgumentError,
      );
    },
  );
}
