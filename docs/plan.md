# Implementation plan and acceptance criteria

## Scope and decisions

Build a standalone native developer tool outside the Zed extension repository. Start with Linux, Windows, and macOS desktop targets in one Rust workspace. Support native debug Flutter apps with explicit instance registration. Keep the UI dense, keyboard-friendly, and responsive under large histories.

Use a modular monolith with ports/adapters and four independently testable crates. Use four Dart integration packages so preferences/Drift are optional dependencies for applications that only need Network.

## Milestones

1. **Foundation** — native shell, bounded background command/event transport, versioned models, SQLite WAL, body files, project license and architecture decisions.
2. **Connection** — authenticated VM URI, isolate discovery/selection, version negotiation, controller lease, reconnect after hot restart.
3. **Network** — Dio public interceptor/adapter integration, raw body capture, error/cancel states, search/table/detail, JSON tree/pretty/raw/hex, request/response/cURL/HAR copy/import/export.
4. **Repeater** — editable persistent drafts, desktop and app execution, redirect choice, finite multipart/binary reuse, cancellation and loop prevention.
5. **Intercept** — host/path/method rules, request and response editing, original forwarding, dropping, mocking, deadlines, cancel/disconnect behavior.
6. **Storage inspectors** — typed preferences CRUD across three APIs, Drift schema/table browser, pagination/filter/sort, SQL, composite-key CRUD, BLOB handling and change notifications.
7. **Validation and delivery** — real Flutter fixture, loopback HTTP tests, real SQLite tests, Rust↔Dart VM integration, native Linux smoke test, benchmark harness, three-OS build matrix, unsigned packaging.

All milestones have implementation in this checkout. Platform release gates remain distinct from local feature implementation: Linux is exercised locally; Windows/macOS jobs must run on their actual CI hosts, and mobile integration must be checked on physical devices before advertising certified support.

## Acceptance invariants

- Network interception never consumes the app's response without forwarding a replacement or the original.
- Capture limits do not truncate data delivered to the app.
- Cancel/timeout/disconnect settle each held request once.
- No inspector extension or Dio wrapping in release/profile builds.
- Saved requests and pins survive restart and retention cleanup.
- Large numbers, duplicate headers, binary payloads, and multipart boundaries remain intact.
- Table mutations use complete primary keys and bound values.
- No network, JSON parse, SQL query, or HAR import runs in the UI draw callback.
- Unsupported/truncated replay is explicit; shell imports are never executed.

## Performance gates

Targets, not assumed achievements:

| Metric | Target | Harness |
| --- | --- | --- |
| First usable window p95 | under 1 s | Native release launch/first-frame instrumentation |
| Idle RSS | under 150 MiB | Local OS process sampling |
| 50,000-row history search p95 | under 100 ms | bench_history, 30 samples |
| Scrolling frame p95 | at most 16.7 ms | Native frame trace on representative machines |

Record the hardware, renderer, build profile, dataset, and measurement definition. A single mapped-window sample does not establish first-paint p95. Software OpenGL/Xvfb results must not be presented as hardware-rendered desktop performance.

## Deliberate boundaries

No Zed fork, Electron/WebView runtime, global TLS MITM, Flutter web, WebSocket replay, CPU/memory profiler, cloud account, or remote history service. Signing, notarization, publishing binaries, and publishing Dart packages are release operations beyond this local implementation.

## Built-in inspection follow-up

Ordinary Network, preferences and Drift inspection now use the runtime’s existing debug APIs. Automatic discovery accepts apps with no Inspector SDK. Desktop Repeater remains available independently; intercept and replay through an app Dio client require the optional bridge. Native adapters are separate from UI/domain code and coexist with explicitly registered providers. Verification includes a real Flutter session with no custom extension, hybrid bridge/native routing, and the desktop UI through auto-reconnection.

## Drift editing update

The Drift pane now separates database/table selection from a virtualized editable grid. State, drawing, and request orchestration live in separate modules. Saves are transactional, row mutations use complete keys, empty-table operations require confirmation, and stale provider replies cannot cross database selections. Linux fixture tests exercise the desktop against two real databases and a large page. Intermittent OS unresponsiveness remains under investigation because the reported dialog has not been reproduced; the measured large-data rendering paths are now bounded.

## Launcher and persistent Logcat dock

The desktop now owns optional Flutter debug runs independently of editors. Projects reads VS Code/Zed launch profiles, remembers folders, resolves installed SDKs/devices, and controls one `flutter run --machine` session. Configuration parsing, machine protocol/process ownership, log streaming/buffering, and desktop rendering stay in separate modules behind application commands/events. Externally started debug sessions remain supported.

Android Logcat is an always-open resizable bottom dock with level colors, app/all scope, filters, manual multi-line clipboard selection, clear and auto-scroll. Both ADB and Flutter output use bounded asynchronous streams and virtualized views. Network now exposes individual/all-history clear with persistent suppression of late updates and body cleanup while preserving Repeater drafts.

## GPS live-tracking simulation

Implemented an Android GPS controller independent of VM observation: native map, coordinates/accuracy/altitude, heading and speed, cardinal step controls, route drawing, GPX/GeoJSON import/export, explicit OSRM driving directions, playback, pause/resume, loop and Stop/Restore. A bundled Java/Dex helper is deployed through ADB with no app-project edits. Domain geometry, application commands, asynchronous actor/process adapter, route I/O and desktop presentation are separate. Device-side locking, a restoration journal and a 10-second host lease handle interruption; Android's mock flag remains visible. Geolocator's default fused stream and the full release desktop flow were verified on an isolated Android emulator; see validation.md for remaining platform coverage.
