# Flutter Inspector

<img src="packaging/icon.svg" width="96" height="96" alt="Flutter Inspector logo">

A native desktop workbench for inspecting Flutter applications: Network, Repeater, Intercept, SharedPreferences, Drift, Android Logcat, GPS simulation, and a Flutter project launcher. Built with Rust and egui, without a browser runtime. MIT licensed.

The desktop targets Linux, Windows, and macOS. Built-in debug APIs inspect Flutter applications on Android, iOS, and desktop. An optional Dart bridge adds interception and replay through the app’s Dio clients. This repository is independent of the Zed extension.

Download the [latest preview](https://github.com/fajarxfce/flutter-inspector/releases/tag/preview) for Windows x64, Linux x64, or macOS (Apple Silicon and Intel). Every successful build of `main` updates this prerelease with unsigned packages and SHA-256 checksums.

## Run the desktop

Install Rust 1.95.0, a native C/C++ toolchain, and the platform graphics libraries. On Ubuntu:

    sudo apt install build-essential pkg-config libx11-dev libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libegl1-mesa-dev libgl1-mesa-dev
    git clone https://github.com/fajarxfce/flutter-inspector.git
    cd flutter-inspector
    cargo run --release -p flutter-inspector

Windows requires the MSVC Build Tools. macOS requires Xcode Command Line Tools.

On Linux/Wayland, redraw pacing uses the compositor callbacks rather than an additional blocking EGL vsync wait. This keeps the event loop responsive when a workspace/window is hidden. Background events continue to be processed while the window is hidden or minimized. X11, Windows and macOS keep their default vsync behavior.

The desktop works offline for stored history, HAR imports, and desktop HTTP replay. **Auto observe** is enabled by default: run your app in debug mode from an IDE or `flutter run` and the Inspector connects when exactly one matching app is available. With multiple apps, select one from the app menu. **Manual URI** remains available for authenticated Dart VM Service connections.

### Run a project without an editor

Open **Projects**, choose the folder containing `pubspec.yaml` (Browse or a typed path), select a configuration/device, and press **Run debug**. The Inspector starts Flutter, connects to its VM automatically, and provides **Hot reload**, **Hot restart**, **Stop**, and searchable/copyable run output. **Recent** keeps the last twelve opened folders across launches. Closing Inspector also stops runs it started; externally launched apps remain independent.

Profiles are read from `.vscode/launch.json` and `.zed/debug.json`, including JSONC comments/trailing commas. Supported fields include `program`, `cwd`, `deviceId`, `toolArgs`, `args`, `flavor`, `env`, and simple `envFile` KEY=value entries. Workspace-folder and environment placeholders are expanded. Arguments remain separate values; no shell command is constructed. Use Launch details to edit Flutter arguments, one per line, for the current run. Editor files remain unchanged. App entrypoint `args` follow Flutter’s current desktop-only support.

Without a launch profile, **Default debug** uses `lib/main.dart`. The SDK picker accepts a folder or executable. Automatic resolution tries `INSPECTOR_FLUTTER`, project `.fvm/flutter_sdk`, VS Code `dart.flutterSdkPath`, `FLUTTER_ROOT`, PATH, then `~/flutter`. FVM must already have installed the SDK. Unresolved editor variables, custom tools/hooks, and incompatible release/profile flags are reported; arbitrary editor tasks are not executed. Standard Flutter startup may run pub get and create normal build artifacts.

### Android emulator / USB

1. Enable USB debugging and authorize the computer on your phone, or start an Android emulator.
2. Run your existing app using **Projects → Run debug**, Zed, another IDE, or `flutter run`. No Inspector dependencies or initialization are needed.
3. Open Inspector with **Auto observe** enabled. No URL copying or manual port forwarding is needed.

Discovery finds ADB through INSPECTOR_ADB, ANDROID_HOME, ANDROID_SDK_ROOT, common Android SDK directories, or PATH. The app menu shows unauthorized/offline device and missing-ADB diagnostics. ADB runs in background without a terminal window.

The Inspector reads VM startup markers from Android's main log buffer, verifies that their processes still exist, preserves the authentication path, and reuses existing port forwards. It also discovers running Dart Development Service (DDS) processes started by your IDE, so a live IDE debug session can be found after Android logs rotate. VM-to-DDS redirects keep their new port and authentication path.

Running apps without the SDK connect through their built-in debug APIs. Connection failures display their reason. Discovery never claims a controller lease. It removes only forwards it created, on normal shutdown or when obsolete. If neither a live DDS session nor a startup marker is available, restart the debug app; endpoints found during a session remain cached while the process is alive.

**Auto observe** checks in the background while disconnected, starting at five seconds and backing off to thirty seconds when nothing changes. Automatic scans keep the toolbar stable, without a spinner replacing the Refresh button. Full process/ADB scans and the discovery repaint timer stop while connecting or connected; the existing VM connection checks liveness with a lightweight heartbeat. A lost connection or hot restart triggers a fresh scan for the selected app. Use **Refresh** to discover apps immediately, including while connected. An explicit **Disconnect** turns auto-observe off. Multiple apps or a bridge owned by another Inspector require selection/availability; discovery never steals the lease.

Local Linux/Windows/macOS IDE sessions are discovered through DDS. Apps with the optional bridge also advertise a short-lived descriptor from its temporary directory. The app and Inspector must share the same user temporary directory; sandboxed desktop apps with separate containers use Manual URI. iOS and remote/network targets currently use **Manual URI**; iOS mDNS discovery is not implemented.

### Simulate GPS for live tracking

Open **GPS**, select an Android emulator or authorized USB phone (Android 8+), turn on its Location setting, choose a point on the map or enter coordinates, then press **Start GPS**. The bundled helper is deployed automatically through ADB; no root, APK installation, Dart VM URL, or Flutter bridge is required. Your app still needs its normal location permissions. GPS is independent of the Flutter debug connection and changes location for the whole selected device.

- **Apply point** teleports to the selected coordinate and holds it. Set altitude/accuracy in meters. **Use live position** copies the current simulated fix into the controls.
- Set heading (0° north, 90° east) and speed in km/h, then **Drive / update direction**. North/East/South/West buttons move by the selected step distance and hold the new position.
- Enable **Add waypoints by clicking map**, click the route, then **Play route**. **Pause/Resume** stays visible above the map. Pausing keeps publishing a stationary fix; reaching the endpoint holds position. Looping jumps from the end to the start; use a closed route for a continuous loop.
- **Get road directions** replaces 2–25 waypoints with an OSRM driving route. It sends those coordinates to the public OSRM service. OpenStreetMap tiles use the internet and a persistent HTTP cache; turn off **Map tiles** for offline coordinates/playback. Imported GPX (one track segment or route) and GeoJSON LineString routes work offline. Files are limited to 2 MiB/5,000 points; export writes GeoJSON. Imported timestamps are ignored: playback uses the selected constant speed.
- **Stop & restore GPS** restores Android's providers and the helper's previous mock-location permission. Closing Inspector also stops the session. USB loss/host failure expires the device lease after 10 seconds. **Restore GPS** recovers the helper's saved journal after an interrupted session; reconnect the device first.

Android reports these fixes as **mock locations** (`Position.isMocked == true` in geolocator). Mock detection is not bypassed. Don't run another mock-location tool at the same time. Device administration/OEM restrictions can reject mock providers; the UI displays the Android error. GPS control currently targets Android only; the desktop still targets Linux, Windows and macOS.

See [GPS internals and tests](docs/gps.md) for the helper protocol and recovery behavior.

### Android app manager

Open **Apps**, select an Android emulator or USB device, and choose **Installed**, **System**, or **All**. Installed means user-installed apps. Search by package name; selecting a package loads its details on the right. The list is scoped to the current Android user and refreshes only when requested.

- **Permissions:** toggle editable runtime permissions, grouped by purpose. Each change is read back from Android. System/policy-fixed, install-time and unknown permission states stay read-only. Separate buttons open Android's special-access settings for notifications, overlays, all-files access and exact alarms.
- **Storage:** clear only the selected app's cache, or clear its storage including local databases, preferences, files and login state. Both show the exact package, device and Android user before confirmation. Cache-only clearing requires Android's `pm clear --cache-only`; older devices offer App info → Storage instead, without falling back to clearing data.
- **App actions:** launch the app, force stop it, or open App info on the device. Storage usage is shown by Android in App info → Storage.

This works independently of a Flutter debug connection and needs no bridge. Android can stop an app when its permission is revoked, and may reject changes based on policy, target SDK or permission dependencies. After changing settings on the phone, use **Refresh details**.

### Android date, time and timezone

Open **Utilities**, select an Android emulator or USB device, and use **Read device time**. Opening the utility only reads settings; it does not need a Flutter debug session or bridge.

- Enter an IANA timezone such as `Asia/Jakarta`, or use the searchable **Browse** list, then **Apply timezone**.
- Click the date to open a calendar and **Save** your selection. Use the 24-hour time picker to choose hour, minute and second, then press **Done**. **Apply date & time** sends the selection to the device. These values use the device's current timezone; apply a new timezone first if needed. Ambiguous/nonexistent daylight-saving times are rejected.
- **Sync with computer** sets the device's instant from the computer's clock while retaining its timezone.
- Manual changes disable the corresponding automatic setting. Use the automatic-setting checkboxes and **Apply automatic settings**, or **Enable both**, to return control to Android.

Changes affect all apps on the selected device and remain after Inspector closes. Each Apply reads the device again to verify the result. Android policies and permissions can reject changes; errors remain visible, and **Open Android settings** provides a manual alternative. Failed writes attempt to restore the settings they changed. There is no periodic ADB clock polling.

### Finding a manual URL

Look in the IDE's Debug Console or Flutter output for “A Dart VM Service … is available at” or “The Dart VM service is listening on”. Copy the entire URL, for example http://127.0.0.1:54321/aBcDe=/, including its token path. Use the VM Service URL, not the separate DevTools browser URL. The fixture below also displays it in its window.

## Try the real Flutter fixture

Use Flutter 3.47.4 / Dart 3.13.3 to reproduce the checked dependency versions:

    flutter pub get
    cd examples/flutter_fixture
    flutter run -d linux --dart-define=INSPECTOR_BRIDGE=false

Use windows or macos as the device on those platforms. Open Inspector and let Auto observe connect to the fixture, then press GET JSON or POST JSON. The fixture also has multipart, binary, large JSON, error, and cancellation scenarios, preferences, and a real SQLite database through Drift. Omit the define to also test the optional bridge, interception, and instance-specific providers.

## Inspect an existing app — no SDK setup

Run the app from an IDE or `flutter run`, then open Inspector:

- Network uses Dart’s HTTP profiler, including standard Dio `IOHttpClientAdapter` traffic. Request/response viewing, copy, HAR and desktop Repeater work without an SDK.
- SharedPreferences uses its built-in DevTools library. Legacy preferences and the default SharedPreferencesAsync store are detected. Custom backend options and a specific WithCache instance need the optional preferences provider; editing the default backing store does not refresh arbitrary app caches.
- Drift databases are detected through Drift’s built-in DevTools registry and service extension. Tables, SQL, row editing and BLOBs work without manual registration.

These adapters were verified with Dart 3.13.3, SharedPreferences 2.5.5 and Drift 2.35.0. Older package APIs may differ. The IDE/Flutter expression compiler must remain running: launching a prebuilt Flutter executable alone does not provide it. Flutter web and release builds are not supported. Drift databases opened or closed after connection update the database picker automatically. Use **Refresh databases** to retry discovery without reconnecting; discovery errors appear in the Drift pane.

## Optional intercept / replay through app Dio

Install these two local packages only when you want to hold/edit live traffic or run Repeater through the app’s existing Dio instance:

    dependencies:
      flutter_inspector_bridge:
        path: ../flutter-inspector/packages/flutter_inspector_bridge
      flutter_inspector_dio:
        path: ../flutter-inspector/packages/flutter_inspector_dio
    dependency_overrides:
      flutter_inspector_bridge:
        path: ../flutter-inspector/packages/flutter_inspector_bridge

Adjust the paths for your app. Import each package’s library and register the existing Dio instance after initialization:

    if (kDebugMode) {
      final bridge = await InspectorBridge.start(name: 'My Flutter app');
      DioInspector(id: 'api', dio: dio, bridge: bridge);
    }

Built-in preferences and Drift remain available with this setup. The additional `flutter_inspector_preferences` and `flutter_inspector_drift` packages are only needed for explicit instance registration or compatibility with older packages. They expose `PreferencesInspector(id: 'prefs', preferences: preferences, bridge: bridge)` and `DriftInspector(id: 'database', database: database, bridge: bridge)`.

Keep the bridge for the app’s lifetime and dispose it with the integration. Use unique provider IDs and register each Dio instance once. A second isolate needs its own bridge. Release/profile builds do not install the custom service or Dio wrapper. The local packages are not published on pub.dev yet.

Auto observe handles Android forwarding. For a manual Android connection, Flutter generally prints a forwarded URI; alternatively:

    adb forward tcp:8181 tcp:<device-vm-service-port>

Use the forwarded localhost port and keep the original URI authentication path. Physical iOS devices need a reachable or forwarded debug VM Service, as with Flutter DevTools.

## Workbench

| Pane | Behavior |
| --- | --- |
| Projects | Open/recent folders, VS Code/Zed debug configurations, device/SDK selection, Run/Stop/hot reload/hot restart, automatic VM connection, run output |
| Logcat (bottom dock) | Always open, resizable height, Android device selection, colored levels, App kita / Semua log, PID/text/minimum-level filtering, Clear, Copy visible, auto-scroll |
| Network | Checkbox selection, Clear selected/Clear item/Clear all, virtualized paged history, method/status/URL filters, Time and Duration columns, header/parameter tables, colored pretty JSON, raw/tree/hex bodies, copy cURL/request/response, pin, HAR import/export |
| Repeater | Multiple independent request tabs with their own saved responses; editable headers/query/body; desktop HTTP or registered app Dio execution; cancellation; raw HTTP/cURL import; binary bodies; redirect choice |
| Intercept | Match host/path/method, hold request and/or response, edit, forward original, drop, mock, expiry countdown |
| Preferences | Search, typed read/write/delete, refresh through built-in APIs or an optional registered instance |
| Drift | Database/table sidebar, editable grid, batch save, insert/delete/empty, filtering/sorting, composite keys, BLOB base64, SQL console |
| Settings | Theme, retention, disk quota, data directory, recent activity |

Network filters combine space-separated terms: m:POST s:4 checkout. JSON numbers retain their original precision. Body work runs on background workers; visible table/tree/hex rows are virtualized.

Right-click any Network row and choose **Send to Repeater**. Each copy opens a new tab with an editable request on the left and its own response on the right. Send runs from the desktop by default; it works without a connected app or bridge. Changes and sends preserve the original capture. Different tabs can send concurrently; each tab retains its latest response. Repeater sends are stored separately from Network history and HAR exports.

Use **+ New tab** or **Paste cURL / HTTP** to start another request. Edit its name to label the tab. Edits save automatically; closing a tab keeps its request and latest response in **Saved requests**, including after restarting Inspector. Right-click an open tab to delete its saved request and response. **Via app Dio** is optional and uses the app's interceptors; update `flutter_inspector_dio` to this version for response correlation. The app's native HTTP profiler may also observe that app-side traffic.

Network detail sits below the history table, with Request and Response visible side by side. Drag the horizontal divider to resize detail and the vertical divider to resize the two messages. The red **×** at the upper right closes detail. Right-click a history row, message heading or body line for Send to Repeater, copy cURL/request/response/URL, pin and Clear item. Copy body copies the full captured text, formatted when Pretty is selected. Each message has body formats (**Pretty**, **JSON tree**, **Raw**, **Hex**) alongside **Headers**; Request also has **Parameters**. Headers and query parameters use Name/Value rows and preserve repeated names. URL-encoded bodies offer **Form fields**. Pretty JSON colors keys, strings, numbers, booleans and null. **Time** shows the local request start time with milliseconds; hover for its date/time zone. **Duration** remains the elapsed request time.

Capture records application-level HTTP bytes. Dio's underlying adapter may already decompress responses; this is not a packet capture. Desktop replay uses the desktop's network environment and trust store. App replay runs application interceptors again, so app logic can change authorization or content. It is tagged to avoid intercept loops.

### Android Logcat

The Logcat dock remains visible below every pane. Drag its top edge to change height; the native window is resizable too. **App kita** filters to the connected Android app's VM PID, matched to its ADB device or the launcher device. A manual PID override is available. **Semua log** includes all PIDs on the selected device. Choose a minimum level and type a case-insensitive filter for messages, tags, or PIDs. Verbose is gray, Debug blue, Info green, Warn amber, Error red, and Fatal purple, with readable alternatives in light mode. Drag to select text across lines and press Ctrl+C (Cmd+C on macOS); selection automatically turns off auto-scroll. This also works in Run output. Right-click a row to copy its full captured line, including text beyond the display preview.

ADB streams continuously through one subprocess. Clear only empties the Inspector buffer; it does not clear the device's log buffers. The panel keeps at most 10,000 lines / 8 MiB of retained text, draws only visible rows, and reports discarded old/overflow lines. Uncheck Auto-scroll to read older output. Device refresh is explicit; connection changes refresh device matching once. An ended stream shows Retry. Logcat is Android-only; other targets still have Flutter run output.

### Clearing Network history

Check individual requests and press **Clear selected (N)** to remove just those captures. The header checkbox selects/deselects the current page. Selection follows request IDs across refreshes, incoming traffic, filters, and pages; the counter includes hidden selections and shows how many are outside the page. **Deselect all** resets selection. Clicking a request still opens its detail independently. Selected history is cleared in one transaction, including checked pins; an empty selection cannot clear history.

Use **Clear item** in a request's right-click menu. **Clear all** asks once and deletes all recorded traffic, including pins and records hidden by filters. Saved Repeater requests and their latest responses stay available; new traffic continues to arrive. Unreferenced response/request body files are removed. IDs of explicitly cleared captures are retained as small deletion markers, so late responses and reconnecting to the same profiler cache cannot restore them. This clears local Inspector history, not another client's VM HTTP cache.

### Editing Drift data

Select a database in the left sidebar, then a table. Its columns and paged data appear on the right. Switching databases clears the old table list and rows immediately; delayed replies cannot overwrite the new selection.

- Double-click a cell to edit. Enter stages it; Tab moves to the next editable column. **Save changes** commits all edited/new rows in one transaction. **Discard** resets pending edits.
- **+ New row** adds an editable row. Omitted cells use SQLite defaults. Right-click a cell to copy, set NULL, reset an edit, or restore an insert default. BLOB editing uses base64.
- Select row checkboxes and use **Delete selected**, then confirm. **Empty table** separately confirms deletion of every row, including filtered or off-page rows, while preserving the table structure.
- Save or discard changes before switching tables/databases, sorting, or paging. The SQL console remains available for explicit queries and statements.

Updates/deletes need a complete, non-null primary key; generated columns and views are read-only. A failed batch rolls back completely. If an edited value changed in the app since loading, the save fails and keeps your edits for review. Foreign-key constraints still apply.

## Storage and limits

Original authorization headers, cookies, and payloads are automatically stored locally as plaintext, as requested for this tool. Settings shows the exact directory. SQLite WAL stores metadata and drafts; separate files store raw bodies.

- Default history retention: 30 days; quota: 5 GiB. Pinned exchanges, saved Repeater requests and their latest responses are protected from retention cleanup.
- Capture limit: 20 MiB per body. Oversize/interrupted captures are marked; exact duplication of a truncated request is refused.
- Optional bridge spool: 128 MiB, at most 64 active captures and 1,000 completed bodies. Metadata events are bounded to 1,000 entries / 8 MiB.
- Tree/pretty JSON: bodies up to 5 MiB; the tree preview contains at most 100,000 nodes. Raw and hex views retain the captured data. A displayed raw line is limited to 4,096 characters; Copy body retains the complete captured text.
- Repeater body editing: up to 512 KiB; syntax colors up to 64 KiB. Larger bodies show a bounded read-only preview and send the preserved bytes. Each saved request retains one latest response, replaced on its next send.
- HAR import: 100 MiB per file. Binary response bodies use standard HAR base64; binary requests use the documented _encoding extension.
- Drift saves: at most 200 changes / 2 MiB per transaction; direct cell editing: 256 KiB. Grid previews show at most 150 characters per visible cell; copying retains the full value.
- SQL query results: 1,000 rows; table pages: 200 rows. Views are read-only in the row editor. Updates/deletes require the full declared primary key; use explicit SQL for tables without one.

Built-in HTTP bodies are downloaded in 64 KiB chunks. Dart’s own in-app HTTP profile cache is separate from the Inspector disk quota and can grow during long sessions. Inspector does not clear another DevTools client’s history. Disconnect restores the profiling setting seen when connecting; restarting the app releases its profile cache. Capture begins when profiling is enabled, so earlier startup requests are not recoverable. Nonstandard Dio adapters may not use dart:io and may require explicit instrumentation.

Interception only operates inside registered Dio clients. Unknown-length request streams, ResponseType.stream responses, and SSE responses pass through without being held. Ordinary finite and bounded multipart bodies can be held. A controller disconnect or hold timeout forwards original bytes; cancellation terminates the hold. Defaults: 120-second hold, 5-second heartbeat, 15-second controller lease.

Flutter web, system-wide TLS interception, WebSocket/SSE replay, CPU profiling, package publication, and release signing are outside this version.

## Development

    python3 tools/check.py
    cargo test -p inspector-adapters --test vm_bridge -- --ignored
    xvfb-run -a cargo test -p inspector-adapters --test native_vm -- --ignored
    INSPECTOR_TEST_NATIVE=1 xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/integration_ui.py
    cargo build -p inspector-adapters --example fixture_query
    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/drift_ui.py target/release/flutter-inspector
    xvfb-run -a -s '-screen 0 1600x1100x24' python3 tools/project_ui.py target/release/flutter-inspector
    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/network_selection_ui.py target/release/flutter-inspector
    python3 tools/wayland_idle.py target/release/flutter-inspector
    cargo run --release -p inspector-adapters --example bench_history -- 50000
    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/smoke_ui.py

When another Flutter app is running, add INSPECTOR_TEST_PICK_FIXTURE=1 to the integration UI test to select its fixture from the discovered app menu (requires Tesseract English data).

Add INSPECTOR_TEST_OBSERVE=1 with INSPECTOR_TEST_NATIVE=1 to check that connected sessions stop full scans, explicit Refresh works, and hot restart reconnects. This uses a slow, isolated ADB shim.

The Drift GUI test requires Xvfb, xdotool, ImageMagick and Tesseract with English language data. It starts an isolated two-database Flutter fixture and never modifies an existing debug app.

The Wayland idle check runs directly inside Hyprland, without Xvfb. It creates an isolated hidden workspace, disables app discovery, streams fake Android logs, and verifies the window can still process events and close on schedule. It leaves existing apps and workspaces alone. This covers compositor presentation waits that Xvfb and UI-callback timing cannot detect.

Run Dart package tests from each package directory so native SQLite build hooks run. The real VM integration test spawns its own Dart fixture and loopback HTTP server.

Architecture, invariants, and implementation milestones are in [docs/architecture.md](docs/architecture.md), [docs/plan.md](docs/plan.md), and [docs/protocol.md](docs/protocol.md). Build/release instructions and measured validation are in [docs/releases.md](docs/releases.md) and [docs/validation.md](docs/validation.md).
