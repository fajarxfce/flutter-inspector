"""Linux smoke check: create a real window, take a screenshot, measure RSS.

Run under xvfb-run for CI. Measurements include the selected display/GL driver.
"""
import json
import os
import sqlite3
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from ui_driver import navigate

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/flutter-inspector").resolve())
output = Path(sys.argv[2] if len(sys.argv) > 2 else "artifacts/smoke")
output.mkdir(parents=True, exist_ok=True)
environment = dict(os.environ)
environment.pop("WAYLAND_DISPLAY", None)
environment.pop("INSPECTOR_VM_URI", None)
environment["INSPECTOR_SMOKE_SECONDS"] = "8"
with tempfile.TemporaryDirectory(prefix="inspector-smoke-") as directory:
    environment["INSPECTOR_DATA_DIR"] = directory
    # Smoke tests must not discover or connect to the developer's running apps.
    with sqlite3.connect(Path(directory) / "inspector.sqlite") as db:
        db.execute("CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL)")
        db.execute("INSERT INTO settings VALUES(1,?)", (json.dumps(dict(
            retentionDays=30, quotaBytes=5 * 1024**3, dark=True, autoObserve=False)),))
    adb = Path(directory) / "adb"
    adb.write_text("#!/bin/sh\nprintf 'List of devices attached\\n'\n")
    adb.chmod(0o755)
    environment["INSPECTOR_ADB"] = str(adb)
    started = time.monotonic()
    with (output / "stderr.log").open("w") as errors:
        process = subprocess.Popen([binary], env=environment, stderr=errors)
        try:
            window = None
            while time.monotonic() - started < 7:
                if process.poll() is not None:
                    errors.flush()
                    detail = (output / "stderr.log").read_text()
                    raise RuntimeError("Inspector exited before showing a window:\n" + detail[-8000:])
                found = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(process.pid)], capture_output=True, text=True)
                if found.returncode == 0:
                    window = found.stdout.splitlines()[0]
                    break
                time.sleep(0.025)
            if window is None:
                raise RuntimeError("No visible Inspector window within 7 seconds")
            mapped_ms = (time.monotonic() - started) * 1000
            time.sleep(1)
            status = Path("/proc") / str(process.pid) / "status"
            rss = next(line.split()[1] for line in status.read_text().splitlines() if line.startswith("VmRSS:"))
            subprocess.run(["import", "-window", window, str(output / "network.png")], check=True)
            # Exercise a second pane and a real native input widget.
            navigate(window, "Repeater")
            time.sleep(0.2)
            subprocess.run(["import", "-window", window, str(output / "repeater.png")], check=True)
            report = {"window_mapped_ms": round(mapped_ms, 2), "idle_rss_mib": round(int(rss) / 1024, 2), "binary_mib": round(Path(binary).stat().st_size / 1024**2, 2), "binary": binary, "note": "Single local sample; window mapping is not first-paint or p95."}
            (output / "metrics.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))
            if process.wait(timeout=12) != 0:
                raise RuntimeError("Inspector smoke process failed")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
