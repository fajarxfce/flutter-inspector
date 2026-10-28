# Local validation

Recorded 2026-09-30 on Linux x86_64 (Arch), Intel Core i7-10870H, 16 GiB RAM. Rust 1.95.0; Flutter 3.47.4 stable; Dart 3.13.3.

## Exercised

- Rust formatting, Clippy with warnings denied, workspace unit and integration tests.
- Dart formatting and analysis with infos treated as failures.
- Core bridge: controller ownership, event bounds, immutable snapshots, body bounds/chunking, decision completion, cancellation, timeout, lease expiry, concurrent initialization, disposal/reinitialization.
- Auto observe: descriptor expiry and loopback validation; Android device/PID/log parsing; real authenticated WebSocket discovery through simulated ADB; startup-log cache; forwarding allocation, reuse, cleanup, and remapping ownership; ambiguous/busy/missing targets and hot-restart target selection. Discovery RPC does not acquire or replace a lease.
- Dio with a real loopback HTTP server: credentials, JSON, request/response edits, mock/drop, cancellation, multipart/binary capture, truncation without truncating the app response, app replay/interceptors, stream behavior, and capture resource cleanup.
- Preferences: typed mutations of a registered legacy instance using the plugin's official mock store. The Linux UI fixture also loads real platform preferences and registers legacy, async, and cache providers.
- Drift: real native SQLite; composite keys, bound values, BLOBs, filtering/sorting, views, single-statement validation, query limits.
- Rust→Dart VM test: automatic local and IDE-process URI discovery, a real separately started DDS instance and verified HTTP redirect, authenticated WebSocket, protocol DTO deserialization, body download, request edit, app replay, lease release.
- Desktop HTTP: binary request/response, duplicate headers, credentials, persistent cancellation state.
- Local store: restart, retention, protected data, paging, future schema rejection, HAR round-trip and missing-body markers.
- AOT release probe: bridge remains disabled and exposes no service.
- Linux desktop release build and Flutter fixture debug build.
- Xvfb smoke and an end-to-end UI run from the Flutter fixture through VM Service into SQLite/body files. Network JSON, saved duplication and desktop replay, preferences, and Drift panes were visually inspected. Screenshots/logs are under artifacts/integration-ui.
- Updated UI integration ran with no INSPECTOR_VM_URI: automatically discovered the Linux Flutter fixture, captured requests, then reconnected and captured a new session after terminating and relaunching the fixture. The Android transport is covered with simulated ADB and a real WebSocket.
- Physical Android discovery: Samsung SM A725F running a Flutter debug app from the IDE. Confirmed VM HTTP 302 to DDS, read-only isolate inspection, and no registered InspectorBridge. Fixed discovery displays the app with Bridge not installed in the native UI. IDE-process discovery still finds this session with ADB disabled for the diagnostic run, proving independence from logcat. Full capture with a bridge installed on a physical Android device is not yet exercised.
- Unsigned Linux tar.gz packaging, under dist.

## Measurements

The launch measurements below were recorded with the first auto-observe implementation, before the DDS redirect and IDE-process discovery fix.

| Measurement | Result | Interpretation |
| --- | --- | --- |
| Release executable | 24.05 MiB | Stripped executable with auto-observe before archive compression |
| Window mapped | 82.90 ms | One local release launch, observed through X11 |
| Idle RSS | 116.36 MiB | Auto observe enabled, no connected app, empty history; Xvfb/software graphics |
| Search p50, 50,000 exchanges | 56.81 ms | 30 release-mode samples |
| Search p95, 50,000 exchanges | 58.74 ms | Combined method/status/URL filter and page load |

Desktop launch measurements include auto-observe; the unchanged history search measurements predate it. These are samples, not cross-platform guarantees. Window mapping is earlier than a verified usable frame and does not establish startup p95. Scrolling/GPU frame p95 has not been measured. Rendering work is bounded, but the 16.7 ms frame target remains a release gate.

Reproduce:

    python3 tools/check.py
    cargo build --release -p flutter-inspector
    cargo run --release -p inspector-adapters --example bench_history -- 50000
    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/smoke_ui.py
    cd examples/flutter_fixture
    flutter build linux --debug
    cd ../..
    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/integration_ui.py

## Not locally verified

Windows/macOS compilation, runtime, and signing; Android provider capture and iOS device connections; file portals on every desktop; certificate-store variants; AppImage on older Linux distributions; macOS app/DMG installation. A native three-OS CI matrix and packaging scripts are included, but the remote workflow has not run from this session.

HAR imports entries sequentially; an invalid later entry may leave earlier valid entries in history. SDK spool eviction or a saturated download queue is reported as unavailable/incomplete capture. A lower quota can temporarily be exceeded by protected data or metadata; saved content is retained.

## Built-in inspection update (2026-10-02)

- Rust workspace: 25 tests passed, plus a targeted regression test for interrupted uploads. Clippy with warnings denied, formatting, and Dart analysis passed.
- Real Linux Flutter session launched by flutter run with INSPECTOR_BRIDGE=false: confirmed the custom extension is absent. Legacy/async preferences support typed writes, reads, removal, interpolation-safe strings and concurrent requests. Drift supports automatic discovery, query, filtered rows, insert/update/delete, BLOB round-trip and primary-key validation.
- Unmodified Dio: captured POST JSON, a 256-byte binary response and a JSON response larger than 1 MiB. Body chunks survive VM string-preview truncation. Disconnect restores the prior HTTP profiling setting.
- The same fixture then initializes only the optional bridge + Dio provider. Backend connection retains built-in preferences and Drift, and native preferences remain readable through the normal worker command path.
- Native UI with no bridge: automatic discovery, request/response JSON tree, duplicate to desktop Repeater, persisted bodies/draft, preferences/table views, and automatic reconnection after restarting the Flutter session passed. Screenshots and result JSON are under artifacts/native-ui/.
- The existing real VM/DDS bridge integration still passes capture, intercept, replay and lease checks.
- Release UI validation also passed the native flow. One local Linux/Xvfb sample: 24.40 MiB executable, 83.85 ms to a mapped window, 118.99 MiB idle RSS. These are single samples, not startup/frame p95.
- Interrupted native uploads are marked incomplete so exact request duplication cannot silently send a partial body.

Run the native integration with xvfb-run -a cargo test -p inspector-adapters --test native_vm -- --ignored. It starts and cleans up its own flutter run session. A standalone prebuilt Flutter executable has no expression compiler; keep the IDE/Flutter tool running for built-in inspection.

Native adapters are verified against Dart 3.13.3, SharedPreferences 2.5.5 and Drift 2.35.0. Custom preference backend options/WithCache objects still require explicit instance registration. At that revision, databases created after connecting needed a reconnect; the 2026-10-28 update below removes that limitation. The Dart VM HTTP cache is separate from desktop retention and can grow until cleared or the app restarts. Windows/macOS runtime and physical-device native inspection remain unverified locally.

## Drift grid and responsiveness update (2026-10-02)

- Rust workspace: 31 tests passed; formatting and Clippy with warnings denied passed. Dart formatting and analysis passed. Drift SDK: 4 tests passed.
- Real native VM integration passed batch insert/update/delete, BLOB values, default inserts, rollback when a later row fails, original-value concurrency checks, confirmed empty-table behavior, retained schema, and two independently discovered databases. SDK tests additionally cover generated columns and foreign-key rejection on empty.
- Editor regressions cover database reset and stale success/error correlation, out-of-order row refreshes, original composite keys, bounded Unicode previews, explicit NULL versus omitted defaults, and visiting a cell without changing its value.
- Linux/Xvfb GUI test uses its own Flutter fixture with the custom bridge disabled. Edit two cells and save together, insert with defaults, delete selected, cancel/confirm empty, switch between databases with distinct tables, load 200 rows containing 64 KiB text each (12.5 MiB), scroll, and switch databases repeatedly. Assertions read the actual fixture database through a helper that refuses other applications. Screenshots and results are under artifacts/drift-ui/.
- Removed full-table cloning and large-cell serialization from each Drift UI frame. Primary-key editability checks borrow values. Worker events are limited per frame so queued work yields back to the window event loop.
- The user's intermittent OS “Application not responding” dialog was not reproduced. These changes fix observed expensive render paths; they do not establish that every source of the reported hang is resolved. GPU/compositor stalls and other unobserved triggers remain possible.

Reproduce the GUI scenario after building the desktop and `cargo build -p inspector-adapters --example fixture_query`:

    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/drift_ui.py target/release/flutter-inspector

Requires xdotool, ImageMagick, and Tesseract English language data. If language data is stored outside the system directory, set TESSDATA_PREFIX accordingly. The test sets INSPECTOR_FRAME_LOG for opt-in timings and closes with Ctrl+Q. Timings measure elapsed time inside the UI callback, including event processing and egui layout. They exclude GPU presentation and time outside the callback; they are not end-to-end frame latency or isolated CPU profiling.

Release grid run: 2,341 UI callbacks, p95 0.63 ms, maximum 3.23 ms during the full scenario above. This is one Linux/Xvfb run and does not establish GPU frame p95 or cross-platform performance.

The final release also passed the existing native Network/Repeater/preferences/reconnect GUI flow: 4 captured exchanges, 6 persisted body files, and a saved draft. This machine had an unrelated Flutter debug session, so `INSPECTOR_TEST_PICK_FIXTURE=1` selected the fixture from the automatically discovered app menu; reconnection after fixture restart was automatic. This test option requires Tesseract English data. Release executable: 24.63 MiB; unsigned Linux archive: 10.22 MiB. The packaged executable hash matches the release binary.


## Auto-observe scheduling update (2026-10-02)

- Periodic full discovery previously ran every five seconds after a completed scan, including during an active connection. It already used background workers, but repeatedly enumerated local processes, spawned ADB, and opened additional VM probes unnecessarily. Native heartbeat also fetched the entire isolate metadata every five seconds.
- Full automatic scans now run only while disconnected, pause while connecting/connected, and become immediately due after an unsolicited disconnect. Stale endpoints are cleared before retrying; preferred app/device metadata survives. Explicit Refresh still scans while connected. Native heartbeat now checks the isolate ID in getVM's small isolate list, detecting hot restart without fetching all loaded libraries.
- Formatting, Clippy and 34 Rust tests passed. New tests cover the scheduling policy, stale target invalidation, connection transition metadata, and an isolate replacement on a still-open VM WebSocket.
- Native GUI integration used the real Flutter fixture plus an isolated ADB shim delayed by two seconds. Over twelve connected seconds with UI interaction it observed zero additional full scans. Manual Refresh caused a scan. In-process Flutter hot restart (R) and full fixture relaunch both reconnected automatically and resumed capture. Existing Network, Repeater, Preferences and Drift steps also passed. Evidence is under artifacts/auto-observe-ui/.
- This verifies removal of unnecessary periodic work and preserved reconnect behavior. It does not prove the exact cause of the user's intermittent OS unresponsiveness dialog, which has not been reproduced in this scenario.

Reproduce with Tesseract English data installed (set TESSDATA_PREFIX if needed):

    INSPECTOR_TEST_NATIVE=1 INSPECTOR_TEST_PICK_FIXTURE=1 INSPECTOR_TEST_OBSERVE=1 xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/integration_ui.py target/release/flutter-inspector

Three read-only full-discovery samples against the connected SM A725F Flutter debug session took 1.68 s, 1.83 s and 1.69 s. These are background scan durations, not measurements of blocked UI time.

The release GUI run passed the same scan/manual-refresh/hot-restart/full-restart checks. It recorded 3,598 UI callbacks, p95 0.37 ms and maximum 3.27 ms. These callback timings exclude presentation and are not end-to-end frame latency.

A separate already-running debug binary was still mapped after its executable had been rebuilt. A two-second live sample showed 78% of one CPU core on its UI thread; a later focused four-second perf sample was dominated by egui/epaint render/layout and supporting allocations. This does not establish the cause of the OS dialog, but the debug process had not received the new scan policy. A user-level Flutter Inspector launcher now points to the optimized release binary. The old process was left running to preserve its UI state.


## Project launcher, Logcat and Network clear (2026-10-02)

- Formatting and Clippy with warnings denied passed. The Rust workspace has 46 passing tests (two existing external-fixture suites remain opt-in). New coverage includes JSONC profiles from both editors, preserving argument boundaries, incompatible flags/unresolved variables, bounded process lines, Logcat parsing/device matching, filtering/retention, stream Clear without respawning ADB, child cleanup, Flutter machine reload/restart/stop, and Inspector shutdown.
- Storage schema 2 upgrades version 1 without losing history. Tests cover individual/all-history clear, pins, body-file cleanup/shared references, late responses/downloads, persisted deletion markers, new traffic and preserved Repeater drafts.
- Windows Job Object ownership code was separately cross-checked for x86_64-pc-windows-gnu. Full Windows/macOS desktop compilation and runtime remain for the existing three-OS CI matrix; that workflow was not launched in this session.
- The isolated Linux GUI test drives real mouse/keyboard input and reads the X11 clipboard. Manual selection across Logcat rows copies with Ctrl+C and remains stable while new logs arrive; selection turns auto-scroll off. Mine/all filtering, continuing after Clear, dock dragging and native window resizing were also verified. Logcat input is a deterministic fake ADB stream; no device logs are cleared.

Reproduce (requires Flutter, Xvfb, xdotool, ImageMagick, Tesseract English data and xclip):

    xvfb-run -a -s '-screen 0 1600x1100x24' python3 tools/project_ui.py target/release/flutter-inspector

Use INSPECTOR_TEST_LOGCAT_ONLY=1 for just the log/clipboard/resize checks. The full test uses an isolated Inspector data directory and launches only examples/flutter_fixture, with the bridge disabled, on Linux.

The full debug GUI run passed project open/device selection, Flutter Run, automatic VM connection, hot reload, hot restart followed by capture in the new session, Stop and fixture-process exit, persisted recents, Network Clear item, cancelled/confirmed Clear all while a filter hides every row, body cleanup, saved draft retention, and new capture after Clear. Evidence is in artifacts/project-ui/. The fixture uses native inspection without the bridge.

The optimized release passed the same complete launcher/log/clear flow. Evidence is in artifacts/project-ui-release/. This run recorded 1,950 UI callbacks, p95 0.61 ms and maximum 3.50 ms. Callback timing excludes GPU presentation and is not end-to-end latency.

The final release also passed the auto-observe regression with the new Logcat dock enabled: zero full scans during twelve connected seconds, explicit Refresh, hot restart reconnect, and full Flutter relaunch reconnect. Existing Network/Repeater/Preferences/Drift UI steps passed. That run recorded 3,637 callbacks, p95 0.42 ms, maximum 3.53 ms. Evidence is in artifacts/auto-observe-ui/.

## Network multi-selection and application logo (2026-10-02)

- Formatting, Clippy with warnings denied and all 48 Rust tests passed. Two external-fixture suites remain opt-in. New storage tests verify selected-ID batches with pins, duplicate/missing IDs, SQL-looking IDs, shared body references, preserved drafts, late updates after reopening, empty selection, and full rollback of records/deletion markers on a failed batch.
- The isolated GUI test seeds 205 captures in a temporary store with discovery disabled and a fake empty ADB device list. It verifies independent request detail, checkbox selection, exact selected deletion, selection stability after prepending newer traffic, selecting/deselecting one page, selection across pages and filters, Deselect all, and the disabled zero-selection clear. The debug build passed; screenshots are under `artifacts/network-selection/`. No existing Flutter app or Inspector history is used.
- The SVG logo is rendered to a bundled PNG, a seven-size ICO (16–256 pixels), and ICNS representations up to 1024 pixels. Container headers, PNG dimensions, Python syntax and the Linux desktop entry validate. The sidebar logo is visible in GUI screenshots. Windows executable resource embedding and macOS bundle integration are configured; native display verification on those OSes remains for CI/manual testing.

Reproduce the selection check (requires Xvfb, xdotool, ImageMagick and Tesseract English data):

    xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/network_selection_ui.py target/release/flutter-inspector

The optimized release passed the same complete selection flow; evidence is under `artifacts/network-selection-release/`. Release binary size is 25.37 MiB. The installed Linux launcher points at this build and uses the new logo.

## Native Wayland idle hang (2026-10-05)

The user's current release was running as a native Wayland client on Hyprland. It was responsive during the initial samples; attaching to that existing process was denied by the kernel ptrace policy. Separate probes reproduced an event-loop stall on a hidden workspace with auto observe disabled and no Flutter app connected. The original release failed to execute its timed close. Main-thread syscall stacks show `ppoll → wl_display_dispatch_queue → libEGL_mesa` repeatedly waiting inside the presentation path. Evidence is under `artifacts/anr/baseline-waits.trace` and `artifacts/anr/wayland-stack.trace`. Earlier Xvfb tests exercised X11, and UI-callback measurements excluded this wait.

Linux/Wayland now disables EGL's blocking swap-interval wait and relies on winit/compositor redraw pacing. X11, Windows and macOS retain their default vsync behavior. Event draining and connection scheduling moved to `App::logic`, so they also run when eframe suppresses hidden-window UI painting. Processing retains its 32-event / 4 ms budget. No device, database, auto-observe interval or global graphics setting was changed.

- Formatting, Clippy with warnings denied and all 48 Rust tests passed (two external-fixture suites remain opt-in).
- The debug build passed Network multi-selection/filter/page/clear regression and the native hidden-window check. With the fix, the separate syscall probe returned to normal event-loop waits and exited cleanly on schedule.
- The release spent 60.08 seconds on an isolated hidden Hyprland workspace, with discovery off and a fake ADB stream. It processed 604 events, exited successfully on schedule, used 1.10 CPU seconds total, and reached 111.1 MiB peak RSS. Recorded callback samples: UI p95 0.68 ms/max 3.92 ms; logic p95 0.034 ms/max 0.082 ms. These samples are not end-to-end presentation latency. Evidence: `artifacts/anr/wayland-idle-release/report.json`.
- The release also passed the full real-Flutter auto-observe regression: no additional full scans during the connected interval, explicit Refresh, hot restart and full-relaunch reconnect, Network/Repeater/preferences/Drift, and persisted request bodies. Only the isolated fixture was used. Evidence: `artifacts/auto-observe-ui/`.

Reproduce the native regression directly inside Hyprland (do not use Xvfb):

    python3 tools/wayland_idle.py target/release/flutter-inspector --seconds 60

The existing user instance was left running to preserve its state; reopening from the installed launcher uses the rebuilt release. This fixes the reproduced hidden-surface stall; it does not establish that every possible source of unresponsiveness has been eliminated.

## GPS simulation (2026-10-05)

Verified on Linux/Hyprland with an Intel Core i7-10870H. The release is built with Rust 1.95.0 and egui/eframe 0.36.2. GPS adds native Walkers 0.60.0 maps; no browser runtime or Flutter bridge is required.

- Formatting, Clippy with warnings denied, 53 Rust tests, Dart workspace analysis and the geolocator fixture analysis passed. Route tests cover cardinal movement, antimeridian crossing, duplicate waypoints, endpoint/altitude interpolation, invalid coordinates/speed, pause/resume, looping, host-delay pause, GPX/GeoJSON round trips and disconnected/oversized route rejection.
- The release UI smoke with fake ADB passed route drawing, explicit Start (opening the pane sends no location), Play/Pause/Resume, background playback across tabs, Stop, stalled-helper timeout while navigation stays responsive, retry, and shutdown cleanup. Callback timing on Xvfb: UI p95 1.02 ms/max 3.84 ms; logic p95 0.009 ms. Evidence: `artifacts/gps-ui/`.
- The real desktop -> ADB -> Android 11 Google APIs emulator -> geolocator 14.1.1 default fused stream passed. Position events report advancing coordinates, 30 km/h converted to m/s, correct bearing, altitude and `isMocked = true`; Stop removes the test providers. Evidence: `artifacts/gps-real-ui/result.json`, `geolocator-driving.png`. UI callback p95 0.49 ms/max 3.67 ms on Xvfb.
- The Android protocol harness additionally verified explicit Stop, stationary fixes with speed zero, EOF cleanup, a silent host's 10-second lease expiry, and recovery from the journal after forcibly killing the Android helper. The physical Samsung Android 16 device was only probed read-only for API/provider availability; its GPS and app source were not modified. Physical-device injection and Windows/macOS runtime behavior still need device/platform testing.
- An OSRM driving request between the Jakarta test points returned a valid 94-point GeoJSON route. Online OSM tiles rendered in the UI. Network multi-selection regression passed with the GPS changes.
- The final release passed a 60.10-second native hidden Wayland run with 603 background events, 1.77 CPU seconds, 111.8 MiB peak RSS and normal timed close. UI callback p95 0.76 ms/max 3.78 ms; logic p95 0.051 ms. Evidence: `artifacts/gps-wayland-idle/`.

Callback measurements exclude GPU presentation and do not establish end-to-end latency or performance on other machines. The isolated Android 16.1 emulator image initially failed in its graphics driver before GPS testing; the stable Android 11 image was used for actual location verification. Test emulators and temporary data are separate from the user's running apps/history.

## Network details and independent Repeater tabs — 2026-10-16

- Rust formatting, Clippy with warnings denied, and all 59 workspace tests passed. New regressions cover response ownership across tabs/sends, stale and cancelled updates, deleted reservations, Network clear, body cleanup and restart persistence. The external Dart VM/DDS bridge suite also passed with correlated Repeater responses and body reads.
- Dio formatting, analysis and all 12 package tests passed. The bridge returns each send's exchange and carries separate tab/run identifiers; ordinary captures remain untagged.
- The Linux release passed `tools/repeater_ui.py` against an isolated schema-2 history and a local HTTP server. It verifies Time/Duration, real pretty-JSON pixel colors and large-number preservation, structured headers/query parameters, the right-click action while a different detail is selected, two concurrent tabs, edited request headers/body, independent 201/202 responses, unchanged original captures, Network clear and saved-response restoration after restarting. Evidence: `artifacts/repeater-ui/result.json` and screenshots in that directory. This flow is included in Linux CI.
- The release also passed the existing 205-row Network selection regression: separate detail/checkbox selection, stable IDs after refresh, paging, hidden selections through filters, batch clear and empty-selection no-op. Evidence: `artifacts/repeater-network-selection/`.
- Native Wayland hidden-window idle with a fake continuous Logcat stream passed for 60 seconds: 603 events drained, UI p95 0.73 ms/max 3.76 ms, logic p95 0.044 ms/max 0.092 ms, approximately 112 MiB peak RSS. These callback timings exclude GPU presentation. Evidence: `artifacts/repeater-wayland-idle/report.json`.
- Linux release executable: 27.47 MiB; unsigned archive: 11.39 MiB. Packaged executable SHA-256 matches the build: `589f7e2f03ff87622c0e54c05889f819a65ae3381c7c70ba6d7214e727401e8a`. Windows/macOS builds remain covered by the configured CI matrix, not local execution in this session.

## Drift discovery and Network detail layout — 2026-10-28

- Rust formatting and all 59 workspace tests passed. The opt-in native VM integration test also passed against the real Flutter fixture, including database creation after connection, the Drift registry notification, discovery through the desktop worker with the optional Dio bridge present, removal after close, and rejection of a closed database ID. Evidence: `artifacts/network-drift-tests.log`, `artifacts/network-drift-native.log`.
- The real Flutter Drift GUI regression passed editing and saving, insertion, deletion, emptying a table, switching databases, and rendering a 12.5 MiB page. Opening a third database after connection adds it to the picker automatically; closing the selected database removes it and loads the remaining database's tables and rows. Evidence: `artifacts/drift-ui/result.json`, `late-database-opened.png`, `late-database-closed.png` in that directory.
- Drift discovery follows VM registry notifications and supports explicit refresh without reconnecting. Changes are coalesced in the existing worker tick; this adds no periodic ADB scan or registry-evaluation timer. Refreshing the same database preserves pending edits, while switching away from a closed database invalidates its old table/row state.
- Network detail now sits below history with Request and Response visible together, independently selectable body formats, structured headers/parameters, resizable dividers, and a red close icon at the upper right. Send to Repeater, copy and individual-clear actions use context menus. Body labels remain manually selectable.
- The Linux release GUI flow passed simultaneous Request/Response display, pretty-JSON pixel colors, structured headers and query parameters, copying a full response, manual body-text selection, closing detail with the red icon, and the right-click action on a row different from the open detail. Two concurrent Repeater tabs produced independent responses while the original captures remained unchanged; results survived Network clear and restart. Evidence: `artifacts/network-detail-release/result.json`.
- The 205-row Network selection regression passed independent detail/checkbox selection, stable selection after incoming traffic and refresh, selections across pages and filters, batch clear, and the empty-selection no-op. Evidence: `artifacts/network-drift-selection.log` and screenshots in `artifacts/network-drift-selection/`.
- Fixture Dart analysis and Clippy with warnings denied passed. Tests use isolated fixture databases, desktop history, local HTTP responses and fake ADB; they do not modify a user's running Flutter app or stored traffic. Windows/macOS execution was not verified locally.
- The final release passed the Network/Repeater GUI flow again after the URL-alignment polish. It also passed a 60.09-second native hidden Wayland run with a continuous fake Logcat stream: 603 events processed, 1.41 CPU seconds and 112.3 MiB peak RSS. UI callback p95 was 0.69 ms (maximum 3.42 ms); logic p95 was 0.035 ms. Timed close completed normally. These measurements exclude GPU presentation and do not establish long-duration or cross-platform performance. Evidence: `artifacts/network-drift-wayland/report.json`.
- Linux release executable: 27.59 MiB. SHA-256: `a0e6a0f72c50d68994dd4e5bffdcdca7144d0c5c2fa226fe44b69e9941047bb3`. The launcher uses `target/release/flutter-inspector`; closing and reopening Inspector loads this build.
