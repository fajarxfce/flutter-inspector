# flutter_inspector_bridge

Debug-only bridge integration for the native Flutter Inspector desktop.

InspectorBridge.start advertises native desktop sessions for Auto observe by default. Set advertise: false to disable these local descriptors. Discovery reads availability without acquiring a controller lease. Android auto-observe uses ADB and the Flutter engine's VM startup log; no extra Dart setup is needed beyond initializing the bridge.

See the [workspace integration guide](../../README.md) and [protocol contract](../../docs/protocol.md). The package is currently consumed through local path dependencies.
