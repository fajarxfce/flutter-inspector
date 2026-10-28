import 'dart:developer' as developer;
import 'dart:io';
import 'package:flutter_inspector_bridge/flutter_inspector_bridge.dart';

Future<void> main() async {
  final bridge = await InspectorBridge.start();
  if (inspectorDebugMode ||
      bridge.enabled ||
      bridge.connected ||
      bridge.beginBody('') != null) {
    throw StateError('Release bridge must remain inert');
  }
  final info = await developer.Service.getInfo();
  if (info.serverUri != null) throw StateError('Unexpected release service');
  await bridge.dispose();
  stdout.writeln('Release bridge is inert');
}
