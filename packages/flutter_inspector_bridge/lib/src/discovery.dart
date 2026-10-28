import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';

/// A short-lived descriptor in the bridge's private temporary directory.
/// Android discovery uses the engine's VM Service log and ADB forwarding.
class DiscoveryAdvertisement {
  DiscoveryAdvertisement({
    required this.directory,
    required this.name,
    required this.sessionId,
    required this.isolateId,
  });

  final Directory directory;
  final String name;
  final String sessionId;
  final String? isolateId;
  Timer? _timer;
  Future<void>? _writing;
  bool _disposed = false;

  void start() {
    if (!(Platform.isLinux || Platform.isMacOS || Platform.isWindows)) return;
    _tick();
    _timer = Timer.periodic(const Duration(seconds: 3), (_) => _tick());
  }

  void _tick() {
    if (_disposed || _writing != null) return;
    _writing = _publish().whenComplete(() => _writing = null);
  }

  Future<void> _publish() async {
    try {
      final uri = (await developer.Service.getInfo()).serverUri;
      if (uri == null || _disposed) return;
      final file = File('${directory.path}/discovery.next');
      await file.writeAsString(
        jsonEncode({
          'version': 1,
          'uri': uri.toString(),
          'isolate': isolateId,
          'name': name,
          'sessionId': sessionId,
          'pid': pid,
          'updatedAt': DateTime.now().millisecondsSinceEpoch,
        }),
      );
      await file.rename('${directory.path}/discovery.json');
    } catch (_) {
      // Discovery must never interrupt application startup or execution.
      // A manual authenticated VM Service URI remains usable.
    }
  }

  Future<void> dispose() async {
    _disposed = true;
    _timer?.cancel();
    await _writing;
    for (final name in ['discovery.json', 'discovery.next']) {
      try {
        await File('${directory.path}/$name').delete();
      } on FileSystemException {
        // Not published or already removed.
      }
    }
  }
}
