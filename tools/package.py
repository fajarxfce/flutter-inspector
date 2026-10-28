"""Create unsigned, platform-native distribution archives from a release build."""
import argparse
from pathlib import Path
import plistlib
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument("--binary")
parser.add_argument("--appimagetool", help="Pinned appimagetool executable supplied by the builder")
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
system = platform.system()
arch = platform.machine().lower()
binary = Path(args.binary) if args.binary else root / "target/release" / ("flutter-inspector.exe" if system == "Windows" else "flutter-inspector")
if not binary.is_file():
    raise SystemExit("Build first: cargo build --release -p flutter-inspector")
dist = root / "dist"
dist.mkdir(exist_ok=True)
name = "flutter-inspector-" + version + "-" + system.lower() + "-" + arch
with tempfile.TemporaryDirectory(prefix="inspector-package-") as temporary:
    stage = Path(temporary)
    if system == "Darwin":
        app = stage / "Flutter Inspector.app"
        executable = app / "Contents/MacOS/flutter-inspector"
        executable.parent.mkdir(parents=True)
        shutil.copy2(binary, executable)
        resources = app / "Contents/Resources"
        resources.mkdir()
        shutil.copy2(root / "packaging/icon.icns", resources / "FlutterInspector.icns")
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "flutter-inspector",
            "CFBundleIdentifier": "dev.flutterinspector.desktop",
            "CFBundleName": "Flutter Inspector",
            "CFBundleIconFile": "FlutterInspector.icns",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": "1",
            "CFBundlePackageType": "APPL",
            "LSMinimumSystemVersion": "11.0",
            "NSHighResolutionCapable": True,
        }))
        shutil.copy2(root / "LICENSE", stage)
        shutil.copy2(root / "README.md", stage)
        (stage / "packaging").mkdir()
        shutil.copy2(root / "packaging/icon.svg", stage / "packaging/icon.svg")
        shutil.copytree(root / "docs", stage / "docs")
        with zipfile.ZipFile(dist / (name + ".zip"), "w", zipfile.ZIP_DEFLATED) as archive:
            for path in stage.rglob("*"):
                if path.is_file():
                    archive.write(path, path.relative_to(stage))
        subprocess.run(["hdiutil", "create", "-volname", "Flutter Inspector", "-srcfolder", str(stage), "-ov", "-format", "UDZO", str(dist / (name + ".dmg"))], check=True)
    elif system == "Windows":
        with zipfile.ZipFile(dist / (name + ".zip"), "w", zipfile.ZIP_DEFLATED) as archive:
            archive.write(binary, "flutter-inspector.exe")
            archive.write(root / "LICENSE", "LICENSE")
            archive.write(root / "README.md", "README.md")
            archive.write(root / "packaging/icon.ico", "flutter-inspector.ico")
            archive.write(root / "packaging/icon.svg", "packaging/icon.svg")
            for path in (root / "docs").glob("*.md"):
                archive.write(path, "docs/" + path.name)
    elif system == "Linux":
        with tarfile.open(dist / (name + ".tar.gz"), "w:gz") as archive:
            archive.add(binary, arcname="flutter-inspector")
            archive.add(root / "LICENSE", arcname="LICENSE")
            archive.add(root / "README.md", arcname="README.md")
            archive.add(root / "docs", arcname="docs")
            archive.add(root / "packaging/icon.svg", arcname="dev.flutterinspector.svg")
            archive.add(root / "packaging/icon.svg", arcname="packaging/icon.svg")
            archive.add(root / "packaging/dev.flutterinspector.desktop", arcname="dev.flutterinspector.desktop")
        if args.appimagetool:
            appdir = stage / "FlutterInspector.AppDir"
            appdir.mkdir()
            shutil.copy2(binary, appdir / "flutter-inspector")
            runner = appdir / "AppRun"
            runner.write_text('#!/bin/sh\ncd "$(dirname "$0")"\nexec ./flutter-inspector "$@"\n')
            runner.chmod(0o755)
            shutil.copy2(root / "packaging/dev.flutterinspector.desktop", appdir)
            shutil.copy2(root / "packaging/icon.svg", appdir / "dev.flutterinspector.svg")
            subprocess.run([args.appimagetool, "--appimage-extract-and-run", str(appdir), str(dist / (name + ".AppImage"))], check=True)
    else:
        raise SystemExit("Unsupported packaging host: " + system)
print("Created unsigned artifacts in", dist)
