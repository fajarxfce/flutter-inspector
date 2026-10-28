# Architecture

## Decision

Use a modular monolith with ports and adapters. One native desktop process owns the UI, background workers, and local store. Built-in VM adapters inspect ordinary debug apps; optional Dart packages instrument application instances for interception and explicit registration.

Rust + egui/eframe 0.36.2 provides native rendering on Linux/X11/Wayland, Windows, and macOS. Glow avoids bundling a browser engine. This is a performance-oriented choice, not a guarantee that every machine meets the benchmark targets.

## Dependency direction

    desktop ──> application ──> domain
       │              ↑
       └──────> adapters ─────> domain

- **domain**: exchange and draft models, protocol DTOs, HTTP import/export helpers, bounded JSON tree representation. No networking, database, or UI imports.
- **application**: command/event contract, backend port, worker lifecycle and bounded channels. No egui, SQLite, VM, or HTTP-client types.
- **adapters**: VM JSON-RPC client, SQLite/raw-body store, HTTP replay, HAR, and the Services implementation of the backend port.
- **desktop**: composition root and feature panes. Emits commands and renders event state; does not perform network I/O or SQL.

The UI modules separate navigation, network/intercept, repeater, preferences, Drift, Logcat, project launching, and body rendering. Drift keeps request/edit state in `drift/state.rs`, virtualized cells in `drift/grid.rs`, and command orchestration in `drift/mod.rs`. A new provider kind extends the versioned bridge protocol and gets its own pane; it does not require a new desktop process or framework.

## Runtime

The UI and background runtime communicate through bounded channels (64 commands, 256 events). At most eight commands execute concurrently; body downloads run two at a time. Disk-heavy reads, JSON parsing/pretty formatting, history queries, HAR, and pruning use blocking worker tasks. Worker events wake the UI on demand. Each UI frame consumes at most 32 events or 4 ms of event processing (at least one event), then schedules another repaint for remaining work.

The VM adapter multiplexes request IDs over an authenticated WebSocket. A selected isolate supplies built-in service extensions and loaded-library evaluation, with an optional custom service extension. The desktop polls HTTP metadata every 350 ms and reads completed body snapshots in 64 KiB chunks. A separate heartbeat task continues while requests are held.

Connection discovery is a separate adapter and command/event flow. It probes local bridge descriptors, Android VM startup markers through ADB, and the VM URI carried by IDE-owned DDS processes without acquiring controller leases. Process enumeration uses sysinfo on a blocking worker and requests command metadata only; it matches Dart development-service commands and loopback URIs. ADB subprocesses have time/output limits and use argument arrays; Windows launches hide the console. Forward ownership is tracked per device/local/remote port, with mappings rechecked before cleanup. Connect/disconnect commands are serialized; a failed old session cannot disconnect a newly selected session. Discovery cancels on shutdown before owned-forward cleanup.

The VM client follows at most five HTTP redirects during websocket setup, normalizing each DDS target to its authenticated websocket URL. Discovery distinguishes an optional bridge, supported built-in debug APIs, unavailable APIs, and connection errors. Both supported modes can auto-connect; the main isolate is preferred over background dart:io isolates. Duplicate targets from multiple discovery sources are collapsed.

While disconnected, the UI schedules discovery five seconds after each scan completes. It does not launch periodic scans while connecting or connected; explicit Refresh still works. An unsolicited disconnect invalidates cached endpoints and makes the next scan immediately due, retaining the preferred app/device. It waits for that fresh scan before reconnecting. Native heartbeat uses the small getVM isolate list, avoiding full getIsolate/library metadata every five seconds; a missing isolate detects hot restart even when the VM socket remains open. It reconnects only an unambiguous available target matching the selected app and device. Explicit disconnect disables auto-observe. Desktop advertisements renew every three seconds, expire after fifteen seconds, and are removed with the bridge's temporary directory. No periodic discovery log is added to the Flutter app.

The desktop stores all completed snapshots and paginates 200 rows. Network detail is a resizable bottom pane with independent Request/Response viewers and a resizable divider. Shared context-menu actions operate on the clicked exchange; body actions wait for the corresponding body ID and are bounded to 32 pending requests. Context menus attach directly to selectable body labels, preserving manual text selection. Request and response bodies share the three-entry body cache with the active Repeater body. JSON is parsed on selection and cached with a small body cache. Rows and tree nodes outside the viewport are not drawn. Drift retains row values by ownership across frames; previews are bounded to 150 characters, and rendering never copies primary-key values. Copying/editing full cells happens only on user action. Replay tasks run separately and retain cancellation handles.

Repeater has stable draft IDs and a unique run ID for every Send. Send/cancel/save/load commands are ordered, while HTTP tasks run concurrently across tabs. Response events must match both the draft and current run; late events cannot overwrite a later send or revive a deleted tab. Desktop sends do not emit Network history refresh events. Optional Dio sends return their own exchange/body descriptors and carry `repeaterId` on bridge capture events, independently of whether Network polling is paused.

Storage schema 3 adds `repeater_results` (one latest exchange per saved draft) and `repeater_bodies` ownership. Network history, retention and HAR export query only `exchanges`. Body cleanup considers both ownership tables, so clearing Network preserves Repeater results. Sending again replaces that tab's previous result and removes orphan bytes; deleting a draft cascades its result. Cancelled runs reject late writes. Existing history/drafts migrate without removal, including old versions' replay rows already stored as Network history.

Only the active Repeater body uses the shared bounded body cache and viewer. Pretty JSON lexing runs over visible, capped lines; the editor highlights payloads up to 64 KiB and offers a bounded read-only preview above 512 KiB. JSON parsing/pretty serialization and URL-encoded body decoding run in the body worker. Repeater sends the original body bytes until explicitly edited; a formatted response is a view, not a request mutation.

## Log streams and project launching

Application-layer DTOs define Android devices/log lines and Flutter launch profiles/control events. The adapters own editor parsing (`project.rs`), Flutter machine protocol/lifecycle (`runner.rs`), ADB streaming (`logcat.rs`), bounded cancellable line decoding (`process_io.rs`), and child-tree lifetime (`process_tree.rs`). UI panes use those contracts and never spawn Flutter/ADB directly. The folder dialog runs off the UI thread.

The Logcat adapter starts one continuous `adb logcat -b all -v threadtime -T 200 *:V` child. Clear advances a generation and discards its pending batch without restarting ADB. Device changes cancel the previous task; stale batches/status from an old generation are ignored. Device discovery is on startup, explicit refresh, or connection changes, not an interval. Matching uses the forwarded VM port or launcher device and getVM's PID. Unknown app identity hides Mine rows until the user connects or supplies a PID.

Both streams batch up to 200 lines / roughly 512 KiB at 100 ms intervals. Nonblocking stream events reserve queue capacity for normal responses; overflow is counted. The UI uses a 10,000-line / 8 MiB ring, incremental filter indexes, and virtualized rows. Log rows have stable IDs, support multi-label text selection/clipboard, and suspend auto-scroll on selection. Clear advances IDs so an old selection cannot attach to new text. Full retained lines can be copied; glyph previews cap at 4,096 characters. ADB lines cap at 64 KiB, Flutter machine records at 256 KiB, and stderr at 64 KiB. No log file is created automatically.

Worker events and connection scheduling run in `eframe::App::logic`, which also executes when UI painting is suppressed for hidden windows. Event draining remains bounded to 32 events / 4 ms per callback. On Linux/Wayland, EGL swap-interval waiting is disabled: winit handles compositor redraw callbacks, while EGL's additional blocking wait could strand the event loop on an unpresented hidden surface. Other platforms retain their normal vsync setting. Opt-in diagnostics time logic and UI callbacks separately; neither metric includes presentation waits.

The project adapter reads JSONC profiles and resolves SDKs off the UI thread. It expands a documented subset of editor variables, rejects control flags that conflict with the machine protocol, and passes argument arrays/environment directly to Flutter. Recent folders live in settings. Run starts `flutter run --machine --debug`, routes app logs/progress to a bounded console, and obtains the VM URI from `app.debugPort` after `app.started`. Reload/restart/stop use correlated machine RPC messages. A managed run suppresses broad auto-discovery and retries its known URI after isolate restart. Explicit Disconnect clears that retry target.

Only one Inspector-owned run is allowed. Stop requests Flutter cleanup first and force-terminates after eight seconds; closing Inspector allows two seconds for cleanup then cancels. Unix children run in a dedicated process group; a Windows Job Object uses kill-on-close. This ownership never includes externally attached apps. Configuration files remain read-only, while Flutter itself performs ordinary pub/build operations in the selected project.

## Built-in adapters

The native module keeps SDK-version-specific code out of the UI and application layer. HTTP uses dart:io profile metadata and bounded evaluation of HttpProfiler bodies. Preferences evaluates its loaded DevTools library; asynchronous results use unique event IDs on a bounded broadcast stream, so concurrent operations do not consume one another’s results. JSON string previews are expanded with getObject before parsing, with a 2 MiB result limit. User strings are escaped for Dart interpolation, never inserted as executable source.

Drift uses its loaded database registry and ext.drift.database protocol. A per-connection VM notification subscriber watches `drift:database-list-changed`, coalescing changes until the worker tick. The UI refreshes the registry using a correlated request, retains a still-open selection, and invalidates table/row/edit state when the selected database closes. Native registry refreshes are serialized and can discover Drift even when its extension was registered after connect. Explicit refresh uses the same path, including registered bridge providers. Drift discovery errors are shown locally without blocking HTTP/preferences connection. SQL identifiers are quoted and schema-checked; values are bound, mutations require complete primary keys, and table-update notifications follow writes. Queries cap results at 1,000 rows and table pages at 200.

Drift grid saves compile schema-validated, bound statements and execute them in one transaction on the actual app database. Updates guard the complete original primary key and edited-cell values. Any failed row rolls back the batch, and notifications are issued inside the transaction so watchers see committed data. Empty-table mutations use DELETE, preserve schema, and obey foreign keys. The optional SDK implements the same contract.

Every Drift request carries a unique correlation tag. Changing database resets schema, rows, edits and all outstanding request tags before rendering the new selection; changing table invalidates row requests. Repeated refreshes supersede earlier responses. Both successful and failed stale replies are ignored. Navigation is disabled while changes await save/discard or a mutation is running.

When the bridge is present, its Dio capture owns network events while detected native preferences/Drift providers remain available. Providers route by ID. Without a registered Dio client, built-in HTTP capture remains active. Native sessions do not acquire a bridge lease. Intercept and app-Dio replay controls reflect available providers.

An IDE or flutter run must supply Flutter’s expression compiler. This is the same runtime evaluation facility used by DevTools, and package-internal APIs may change across versions. Raw standalone Flutter binaries do not supply the compiler. The HTTP profiling setting is restored on disconnect; the shared VM profile cache is never cleared automatically.

## Optional Dart integration

The core bridge owns provider registration, controller leases, events, bounded body files, and the intercept decision queue. It imports no Flutter, Dio, preferences, or Drift dependency.

- Dio: an interceptor associates request metadata/errors, and an adapter wrapper observes transformed outgoing bytes and incoming response streams. App requests use the original adapter and its TLS/timeouts. Streaming capture tees bytes; finite interception buffers only up to the configured cap.
- Preferences: the provider calls the registered preferences object. Legacy and cached APIs reload before reading to expose external changes. Types are checked before mutations.
- Drift: the provider calls the actual GeneratedDatabase. SQL identifiers are quoted and verified, values are bound, and table notifications follow mutations. Composite primary keys identify editable rows.

All integrations are disabled outside debug mode. Disposal restores the original Dio adapter and removes its interceptor without closing the user's Dio client or database.

## Storage

SQLite WAL stores exchange metadata, body references, drafts, settings, and schema version. Raw body filenames are SHA-256 hashes of opaque IDs; remote IDs cannot escape the data directory. Files are written through a temporary name before being recorded in SQLite.

Schema version 2 adds explicit-clear deletion markers and upgrades version 1 without replacing history. History responses carry request IDs; Clear invalidates older pages/body results. Clearing records removes unreferenced body files, and late downloads refuse bodies with no remaining references. Tombstones prevent the source profiler cache from restoring cleared IDs on updates/reconnect. Drafts and settings survive.

Network checkbox selection is a set of exchange IDs, separate from the detail pane. Page selection only changes IDs on the loaded page; refreshes and filters do not remap selection by row position. `HistoryTarget` explicitly distinguishes all history from a set of IDs, including an empty no-op set. The backend snapshots the target, deletes its records and writes deletion markers in one transaction, then cleans up orphan bodies once. UI selection is removed only after success and stays available on failure.

The store rejects future schema versions. Retention never removes pins/drafts. Quota cleanup removes oldest unpinned exchanges and orphan bodies. Protected data can exceed a newly lowered quota; it remains intact and the UI reports the condition.

OS application-data directories come from the directories crate. INSPECTOR_DATA_DIR overrides the path for isolated development and tests. Unix data directories are restricted to their owner.

## Extension rules

Keep public protocol changes additive within major version 1. Changing required fields or semantics requires a new protocol major. Add protocol/adapter tests for behavior crossing processes; avoid tests that merely duplicate widget layout code.

Do not add dynamic binary plugins until real third-party extension needs justify the ABI, permissions, and compatibility cost. New inspectors can first be ordinary Dart providers plus desktop modules.

## GPS simulation

`domain/gps.rs` owns validated coordinates, geodesic movement and route sampling; `application/gps.rs` defines commands/status events; `adapters/gps/` owns a bounded actor, one persistent ADB helper process, route file parsing and OSRM HTTP; `desktop/gps.rs` owns controls and the native Walkers map. Map setup/downloads, ADB, dialogs and parsing run outside the window thread. The GPS actor stays independent of VM polling/discovery and uses a device-side lease for cleanup if its host disappears. See `docs/gps.md` for the protocol and recovery journal.
