"""Exercise the bundled helper against geolocator on an explicitly selected test emulator.

Build/install examples/gps_fixture first. This changes the test emulator's location.
Usage: python3 tools/gps_android_test.py emulator-5580
"""
import json
from pathlib import Path
import selectors
import subprocess
import sys
import time

root = Path(__file__).resolve().parents[1]
serial = sys.argv[1]
assert serial.startswith("emulator-"), "Use an isolated emulator, never a personal device"
adb = ["adb", "-s", serial]
invoke = "CLASSPATH=/data/local/tmp/flutter-inspector-gps-v1.jar app_process /system/bin dev.flutterinspector.gps.Main"


def shell(*args):
    return subprocess.check_output(adb + ["shell", *args], text=True, timeout=15).strip()


def read(process):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        assert selector.select(8), "Helper response timed out"
    line = process.stdout.readline()
    assert line, "Helper exited without a response"
    result = json.loads(line)
    assert "error" not in result, result
    return result


def start():
    process = subprocess.Popen(adb + ["shell", invoke], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True)
    assert read(process).get("ready"), "Not ready"
    return process


def fix(process, longitude, heading=90, speed=7):
    point = dict(latitude=-6.1754, longitude=longitude, altitude=42, accuracy=5, speed=speed, heading=heading)
    process.stdin.write(json.dumps(dict(op="fix", fix=point)) + "\n")
    process.stdin.flush()
    assert read(process).get("ok")


def stop(process):
    process.stdin.write('{"op":"stop"}\n')
    process.stdin.flush()
    assert read(process).get("stopped")
    process.wait(timeout=5)
    assert process.returncode == 0


subprocess.run(adb + ["push", str(root / "crates/adapters/assets/gps-helper.jar"),
    "/data/local/tmp/flutter-inspector-gps-v1.jar"], check=True, capture_output=True)
shell("cmd", "location", "set-location-enabled", "true")
original_mode = shell("cmd", "appops", "get", "com.android.shell", "android:mock_location")
shell("am", "force-stop", "dev.flutterinspector.gps_fixture")
shell("am", "start", "-n", "dev.flutterinspector.gps_fixture/.MainActivity")
time.sleep(4)
pid = shell("pidof", "dev.flutterinspector.gps_fixture")
assert pid, "GPS fixture did not start"
process = None
try:
    process = start()
    for i in range(20):
        fix(process, 106.8272 + i * .00005)
        time.sleep(.5)
    for _ in range(6):
        fix(process, 106.8282, heading=180, speed=0)
        time.sleep(.5)
    stop(process)
    process = None
    logs = subprocess.check_output(adb + ["logcat", "-d", "--pid=" + pid], text=True, timeout=10)
    fixes = [json.loads(line.split("INSPECTOR_GPS_FIX ", 1)[1]) for line in logs.splitlines() if "INSPECTOR_GPS_FIX " in line]
    assert any(abs(p["latitude"] + 6.1754) < .00001 and abs(p["speed"] - 7) < .01 and p["heading"] == 90 and p["mock"] for p in fixes), fixes
    assert any(p["speed"] == 0 and p["heading"] == 180 and p["mock"] for p in fixes), fixes
    assert all(abs(p["altitude"] - 42) < .1 for p in fixes if p["mock"]), fixes
    assert not json.loads(shell(invoke + " --probe"))["recoveryNeeded"]
    print("PASS: Geolocator fused stream received movement, bearing, altitude, stationary fix, and mock flag", flush=True)

    process = start()
    fix(process, 106.8272)
    # Keep stdin open, but stop sending the heartbeat. Device must self-restore.
    process.wait(timeout=14)
    process = None
    assert not json.loads(shell(invoke + " --probe"))["recoveryNeeded"]
    mode = shell("cmd", "appops", "get", "com.android.shell", "android:mock_location")
    if "No operations." in original_mode or "default" in original_mode:
        assert "allow" not in mode.lower(), mode
    print("PASS: Stop and expired 10-second lease restore GPS and mock app-op", flush=True)

    for mode in ["EOF", "SIGKILL"]:
        process = start()
        fix(process, 106.8272)
        if mode == "EOF":
            process.stdin.close()
            process.wait(timeout=6)
        else:
            rows = [line.split(maxsplit=2) for line in shell("ps", "-A", "-o", "PID,NAME,ARGS").splitlines()]
            matches = [row[0] for row in rows if len(row) == 3 and "app_process" in row[1] and "dev.flutterinspector.gps.Main" in row[2]]
            assert len(matches) == 1, "Cannot identify this test helper"
            shell("kill", "-9", matches[0])
            process.wait(timeout=5)
            assert json.loads(shell(invoke + " --probe"))["recoveryNeeded"]
            assert json.loads(shell(invoke + " --restore"))["ok"]
        process = None
        assert not json.loads(shell(invoke + " --probe"))["recoveryNeeded"]
        print("PASS:", mode, "GPS cleanup/recovery", flush=True)
finally:
    if process is not None:
        process.stdin.close()
        try:
            process.wait(timeout=14)
        except subprocess.TimeoutExpired:
            process.kill()
    shell(invoke + " --restore")
