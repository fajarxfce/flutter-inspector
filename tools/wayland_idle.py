"""Native Wayland regression: idle on a hidden Hyprland workspace with Logcat.

Run inside Hyprland, without xvfb-run:
    python3 tools/wayland_idle.py target/release/flutter-inspector

The temporary instance has its own data and fake ADB. It never discovers a real
Flutter app, changes focus, or moves the user's windows. A blocked EGL swap used
to prevent the timed close, even with auto observe disabled.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", nargs="?", default="target/debug/flutter-inspector")
parser.add_argument("--seconds", type=int, default=30)
args = parser.parse_args()
if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE") or not os.environ.get("WAYLAND_DISPLAY"):
    raise SystemExit("Run this check in a native Hyprland/Wayland session, without Xvfb.")
if not 5 <= args.seconds <= 300:
    raise SystemExit("--seconds must be between 5 and 300")
binary = Path(args.binary).resolve()
root = Path(__file__).resolve().parents[1]
out = Path(os.environ.get("INSPECTOR_UI_OUTPUT", str(root / "artifacts/wayland-idle")))
out.mkdir(parents=True, exist_ok=True)

with tempfile.TemporaryDirectory(prefix="inspector-wayland-idle-") as temporary:
    data = Path(temporary)
    with sqlite3.connect(data / "inspector.sqlite") as db:
        db.execute("CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL)")
        db.execute("INSERT INTO settings VALUES(1,?)", (json.dumps(dict(
            retentionDays=30, quotaBytes=5 * 1024**3, dark=True, autoObserve=False)),))
    adb = data / "adb"
    adb.write_text(f"#!{sys.executable}\n" + '''import sys, time
if sys.argv[1:2] == ["devices"]:
    print("List of devices attached\\nfake-phone device model:Idle_Test")
elif "logcat" in sys.argv:
    while True:
        for level in ["V", "D", "I", "W", "E", "F"]:
            print(f"10-05 12:34:56.123 4242 4242 {level} IdleTest: deterministic background log", flush=True)
        time.sleep(0.1)
''')
    adb.chmod(0o755)
    # The runner owns the app process, so timeout cleanup never targets a user's app.
    runner = data / "runner.py"
    runner.write_text('''import json, resource, subprocess, sys, time
from pathlib import Path
data = Path(sys.argv[2])
started = time.monotonic()
with (data / "desktop.log").open("w") as log:
    child = subprocess.Popen([sys.argv[1]], stdout=log, stderr=log, start_new_session=True)
    (data / "pid").write_text(str(child.pid))
    code = child.wait()
usage = resource.getrusage(resource.RUSAGE_CHILDREN)
(data / "exit.json").write_text(json.dumps(dict(
    returncode=code, elapsed_seconds=time.monotonic() - started,
    cpu_seconds=usage.ru_utime + usage.ru_stime, max_rss_kib=usage.ru_maxrss)))
''')
    command = [
        "env", "-u", "INSPECTOR_VM_URI", f"INSPECTOR_DATA_DIR={data}",
        f"INSPECTOR_ADB={adb}", f"INSPECTOR_SMOKE_SECONDS={args.seconds}",
        f"INSPECTOR_FRAME_LOG={data / 'frames.json'}",
        sys.executable, str(runner), str(binary), str(data),
    ]
    workspace = f"special:inspector-idle-{os.getpid()}"
    pid = None
    try:
        subprocess.run(["hyprctl", "dispatch", "exec",
            f"[workspace {workspace} silent] " + shlex.join(command)], check=True, capture_output=True)
        deadline = time.monotonic() + 10
        while not (data / "pid").exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        pid = int((data / "pid").read_text())
        deadline = time.monotonic() + 10
        client = None
        while time.monotonic() < deadline:
            clients = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"]))
            client = next((c for c in clients if c["pid"] == pid), None)
            if client:
                break
            time.sleep(0.1)
        assert client and not client["xwayland"], "Probe must use native Wayland"
        assert client["workspace"]["name"] == workspace, "Probe must stay on its isolated workspace"
        deadline = time.monotonic() + args.seconds + 10
        while not (data / "exit.json").exists() and time.monotonic() < deadline:
            time.sleep(0.2)
        assert (data / "exit.json").exists(), "Hidden window blocked the event loop; timed close did not run"
        report = json.loads((data / "exit.json").read_text())
        assert report["returncode"] == 0, report
        assert args.seconds <= report["elapsed_seconds"] < args.seconds + 10, report
        timings = json.loads((data / "frames.json").read_text())
        assert timings["events_processed"] >= args.seconds * 3, "Background events stopped being drained"
        report.update(timings=timings, backend="Wayland", hidden=True, auto_observe=False)
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Native Wayland idle passed:", json.dumps(report))
    finally:
        if pid and Path(f"/proc/{pid}/environ").exists():
            environment = Path(f"/proc/{pid}/environ").read_bytes()
            if f"INSPECTOR_DATA_DIR={data}\0".encode() in environment:
                os.killpg(pid, signal.SIGTERM)
                deadline = time.monotonic() + 5
                while not (data / "exit.json").exists() and time.monotonic() < deadline:
                    time.sleep(0.1)
        if (data / "desktop.log").exists():
            (out / "desktop.log").write_bytes((data / "desktop.log").read_bytes())
