"""Build the small Android shell helper; ordinary desktop builds use the bundled JAR."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--sdk", type=Path, default=Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Android/Sdk"))))
args = parser.parse_args()
platform = args.sdk / "platforms/android-36/android.jar"
build_tools = args.sdk / "build-tools/36.0.0"
with tempfile.TemporaryDirectory(prefix="inspector-gps-build-") as temporary:
    build = Path(temporary)
    classes = build / "classes"
    classes.mkdir()
    sources = sorted((root / "tools/android-gps/src").rglob("*.java"))
    subprocess.run(["javac", "--release", "8", "-classpath", str(platform), "-d", str(classes), *map(str, sources)], check=True)
    subprocess.run([str(build_tools / "d8"), "--min-api", "26", "--lib", str(platform), "--output", str(build),
        *map(str, sorted(classes.rglob("*.class")))], check=True)
    destination = root / "crates/adapters/assets/gps-helper.jar"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as jar:
        entry = zipfile.ZipInfo("classes.dex", date_time=(2026, 1, 1, 0, 0, 0))
        entry.compress_type = zipfile.ZIP_DEFLATED
        jar.writestr(entry, (build / "classes.dex").read_bytes())
    print("Built", destination, destination.stat().st_size, "bytes")
