# Build and release

Local source implementation does not imply verified releases on every target. The CI matrix builds natively on Linux, Windows, and macOS. No signing credentials or publication destinations are configured.

## Native build

    cargo build --release -p flutter-inspector --locked
    python3 tools/package.py

Output:

- Linux: tar.gz with the executable, license, README/docs, SVG icon and desktop entry.
- Windows: ZIP with the executable (embedded icon), ICO, license and README/docs.
- macOS: app bundle with an ICNS icon, ZIP and unsigned DMG, built by hdiutil on macOS.

The master logo is `packaging/icon.svg`. Run `python3 tools/generate_icons.py` after changing it (requires `rsvg-convert`, provided by librsvg). The generated PNG, multi-size ICO and ICNS are checked in, so ordinary builds need no graphics tooling. The desktop embeds the PNG and loads its sidebar texture once; Windows builds embed the ICO with the platform resource compiler, and the macOS bundle declares `CFBundleIconFile`.

The Linux desktop entry uses app ID `dev.flutterinspector`. Install the executable on PATH, the desktop entry in `~/.local/share/applications/`, and the SVG as `~/.local/share/icons/hicolor/scalable/apps/dev.flutterinspector.svg`, or set absolute Exec/Icon paths in a local desktop entry.

An optional Linux AppImage can be generated with a builder-supplied appimagetool:

    python3 tools/package.py --appimagetool /absolute/path/to/appimagetool

The script does not download a packaging executable. Pin and verify that tool in the release builder. AppImage includes the application, not every graphics driver/system library; test it on the oldest intended distribution. Build Linux artifacts on that baseline distribution because glibc compatibility follows the build host.

The CI jobs upload build artifacts only. Publishing releases, package publication, macOS signing/notarization, and Windows Authenticode are separate release steps.

## Release checklist

1. Run tools/check.py; run the real VM fixture test explicitly.
2. Check the native UI and fixture on each desktop OS with both light/dark appearance, clipboard, file dialogs, reconnect, and display scaling.
3. Run mobile integration on Android USB and an iOS device, preserving VM URI authentication.
4. Exercise authenticated HTTPS with each intended device/desktop trust store. Inspector never disables certificate validation.
5. Measure startup, memory, history search, and scrolling in release mode on representative hardware.
6. Inspect unsigned packages and establish signing/publication separately.

Current local results and unverified gates are recorded in validation.md.
