"""Run checks from their package roots, including Dart native asset hooks."""
from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parents[1]

def run(arguments, directory=root):
    print("+", " ".join(arguments), flush=True)
    subprocess.run(arguments, cwd=directory, check=True, shell=os.name == "nt")

run(["cargo", "fmt", "--all", "--check"])
run(["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"])
run(["cargo", "test", "--workspace", "--locked"])
run(["flutter", "pub", "get"])
run(["dart", "format", "--output=none", "--set-exit-if-changed", "packages", "examples/flutter_fixture/lib", "examples/gps_fixture/lib"])
run(["dart", "analyze", "--fatal-infos"])
for package in ["bridge", "dio", "drift"]:
    run(["dart", "test"], root / "packages" / ("flutter_inspector_" + package))
run(["flutter", "test"], root / "packages/flutter_inspector_preferences")
run(["cargo", "test", "-p", "inspector-adapters", "--test", "vm_bridge", "--", "--ignored"])
(root / "artifacts").mkdir(exist_ok=True)
probe=root / "artifacts" / ("release-probe.exe" if os.name == "nt" else "release-probe")
run(["dart", "compile", "exe", "packages/flutter_inspector_bridge/test/fixtures/release_probe.dart", "-o", str(probe)])
run([str(probe)])
