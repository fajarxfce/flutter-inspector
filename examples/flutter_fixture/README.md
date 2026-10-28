# Flutter Inspector fixture

Run `flutter pub get` from the repository root, then `flutter run -d linux --dart-define=INSPECTOR_BRIDGE=false` from this directory (or choose your Windows, macOS, Android, or iOS device). Keep flutter run active so its expression compiler is available.

Open Inspector with **Auto observe** enabled and use the fixture buttons to generate real traffic. Preferences and Drift are discovered without SDK registration. Use **Read edited storage** to see changes made from the desktop.

Omit the dart-define to additionally enable the optional bridge, Dio interception, and explicitly registered legacy/async/cache preferences providers. The fixture uses a loopback HTTP server, in-memory Drift database, and real platform preferences. All Inspector registration is guarded by kDebugMode.
