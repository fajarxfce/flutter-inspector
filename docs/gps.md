# Android GPS helper

`src/dev/flutterinspector/gps/Main.java` builds a small DEX-only JAR embedded in the Rust adapter. Standard desktop builds need no Android SDK or JDK. Rebuild with `python3 tools/build_gps_helper.py` (JDK, Android SDK platform 36 and build-tools 36.0.0). The generated asset is `crates/adapters/assets/gps-helper.jar`.

The desktop pushes the embedded JAR to `/data/local/tmp/flutter-inspector-gps-v1.jar`, then opens one `adb -s SERIAL shell ... app_process ...` stream. No shell interpolation contains user coordinates or routes: fixes travel as bounded JSON lines over stdin. The helper uses the shell package context and public LocationManager test-provider APIs. Android 8+ supplies latitude/longitude/altitude, horizontal/vertical accuracy, bearing/bearing accuracy, speed/speed accuracy and fresh wall/elapsed timestamps for GPS and network providers. Android retains the mock flag. Fused clients can consume those provider updates; OEM restrictions are returned as errors.

The helper obtains an exclusive device lock before simulation/recovery. Before changing providers, it journals the shell package's mock app-op mode to `/data/local/tmp/flutter-inspector-gps-state.json`, then temporarily permits mock location. `stop`, stdin EOF, and a 10-second heartbeat lease restore providers/app-op and remove the journal. A killed helper or powered-off phone cannot execute cleanup; the next start or explicit **Restore GPS** reads the journal and retries restoration. Recovery only acts when an Inspector journal exists. Concurrent third-party mock tools are unsupported.

Protocol 1 starts with `{"ready":true,"protocol":1}`. Host messages are `{"op":"fix","fix":{"latitude":-6.1754,"longitude":106.8272,"altitude":10,"accuracy":5,"speed":7,"heading":90}}` or `{"op":"stop"}`. Each has one JSON acknowledgement. Speed is m/s and bearing is clockwise degrees from north. Errors return `{"ok":false,"error":"..."}`. `--probe` is read-only; `--restore` recovers a stale journal; `--watch` is a LocationManager consumer for integration tests.

The Rust GPS actor serializes commands in a bounded queue, publishes fixes every 500 ms and waits for each acknowledgement. Deployment and process I/O are bounded by timeouts. Movement uses great-circle distances/bearings and constant-speed route sampling. Pausing/end-of-route publishes speed zero; a host delay over three seconds pauses playback to prevent catch-up jumps. UI state and asynchronous route/device responses carry generations/request IDs. Map downloading/decoding, file dialogs, route parsing, and ADB work do not run on the window thread.

## Verification

Run `cargo test --workspace --locked` and `cargo clippy --workspace --all-targets --locked -- -D warnings`. Native UI/actor/protocol smoke test with fake ADB:

```
cargo build -p flutter-inspector
xvfb-run -a -s '-screen 0 1600x1100x24' python3 tools/gps_ui.py
```

To verify the actual default geolocator fused stream, build and install `examples/gps_fixture` on an **isolated test emulator** with location permission granted. Then run `python3 tools/gps_android_test.py emulator-5580`, substituting its serial. This moves the emulator to Jakarta, verifies latitude/longitude/speed/heading/altitude/mock fixes, stationary updates, explicit Stop, EOF, lease expiry and recovery after killing the helper. The tool refuses physical-device serials.

To exercise the release desktop controls against that same geolocator fixture, run `xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/gps_real_ui.py emulator-5580 target/release/flutter-inspector`. Its ADB wrapper exposes only the explicitly selected test emulator. This verifies Start, heading/speed movement, fused Position events and Stop restoration through the real Rust adapter.
