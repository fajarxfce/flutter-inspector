"""Native utility and GPS-marker checks with isolated history and fake ADB.

xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/device_clock_ui.py [binary]
"""
from pathlib import Path
from datetime import datetime
from zoneinfo import ZoneInfo
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import click, click_text, enter, key, locate, navigate, screenshot, wait_for, window

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/debug/flutter-inspector"
out = Path(os.environ.get("INSPECTOR_UI_OUTPUT", root / "artifacts/device-clock-ui"))
out.mkdir(parents=True, exist_ok=True)
(out / "failure.png").unlink(missing_ok=True)
probe = out / "probe.png"
with tempfile.TemporaryDirectory(prefix="inspector-clock-ui-") as temporary:
    data = Path(temporary)
    adb = data / "adb"
    shutil.copyfile(root / "tools/fixtures/clock_adb.py", adb)
    adb.chmod(0o755)
    initial = dict(epoch_millis=1793150000000, timezone="Asia/Jakarta", auto_time=True, auto_time_zone=True)
    (data / "state.json").write_text(json.dumps(initial))
    (data / "other.json").write_text(json.dumps(dict(initial, timezone="Pacific/Auckland")))
    with sqlite3.connect(data / "inspector.sqlite") as db:
        db.execute("CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL)")
        db.execute("INSERT INTO settings VALUES(1,?)", (json.dumps(dict(autoObserve=False,dark=True,retentionDays=30,quotaBytes=5*1024**3)),))
    env = dict(os.environ, INSPECTOR_DATA_DIR=str(data), INSPECTOR_ADB=str(adb), XDG_CONFIG_HOME=str(data / "config"))
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("WAYLAND_SOCKET", None)
    env.pop("INSPECTOR_VM_URI", None)

    def state():
        return json.loads((data / "state.json").read_text())

    def mutations():
        if not (data / "calls.jsonl").exists():
            return []
        calls = [json.loads(s) for s in (data / "calls.jsonl").read_text().splitlines()]
        return [c for c in calls if any(word in c for word in ["put", "set_auto_detection_enabled", "suggest_manual_time", "suggest_manual_time_zone", "set-time", "set-timezone"])]

    with (out / "desktop.log").open("w") as log:
        app = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        win = None
        try:
            win = wait_for(lambda: window(pid=app.pid))
            subprocess.run(["xdotool", "windowsize", win, "1440", "1000"], check=True)
            navigate(win, "Utilities")
            wait_for(lambda: locate(win, probe, "Device time read"))
            assert not mutations(), "Opening the utility must never change device settings"
            screenshot(win, out / "initial.png")
            timezone = locate(win, probe, "Timezone ID")
            enter(win, timezone[0]+160, timezone[1], "Asia/Kathmandu")
            click_text(win, probe, "Apply timezone")
            wait_for(lambda: not state()["auto_time_zone"] and state()["timezone"] == "Asia/Kathmandu")
            wait_for(lambda: locate(win, probe, "Timezone applied"))
            assert state()["auto_time"]
            section = locate(win, probe, "Date & time", minimum_y=300)
            date = locate(win, probe, "Date", minimum_y=section[1]+35)
            enter(win, date[0]+95, date[1], "2027-02-30")
            count = len(mutations())
            click_text(win, probe, "Apply date & time")
            assert len(mutations()) == count, "Invalid date must not mutate the device"
            enter(win, date[0]+95, date[1], "2027-01-02")
            enter(win, date[0]+265, date[1], "15:04:05")
            click_text(win, probe, "Apply date & time")
            expected = int(datetime(2027,1,2,15,4,5,tzinfo=ZoneInfo("Asia/Kathmandu")).timestamp()*1000)
            wait_for(lambda: state()["epoch_millis"] == expected)
            wait_for(lambda: locate(win, probe, "Date & time applied"))
            assert not state()["auto_time"]
            screenshot(win, out / "manual-time.png")
            # OCR often reads Sync as Syne. Use the fixed form row instead.
            click(win, date[0]+500, date[1])
            wait_for(lambda: abs(state()["epoch_millis"] - int(time.time()*1000)) < 5000)
            wait_for(lambda: locate(win, probe, "Date & time applied"))
            click_text(win, probe, "Enable both")
            wait_for(lambda: state()["auto_time"] and state()["auto_time_zone"])
            wait_for(lambda: locate(win, probe, "Automatic settings verified"))
            # A stalled ADB command must yield to normal window navigation and recover.
            (data / "hang").touch()
            click_text(win, probe, "Read device time")
            navigate(win, "Network")
            wait_for(lambda: locate(win, probe, "Waiting for traffic"), seconds=3)
            navigate(win, "Utilities")
            wait_for(lambda: locate(win, probe, "timed out"))
            (data / "hang").unlink()
            click_text(win, probe, "Read device time")
            wait_for(lambda: locate(win, probe, "Device time read"))
            click_text(win, probe, "Clock Test")
            click_text(win, probe, "Other Device")
            wait_for(lambda: locate(win, probe, "Pacific/Auckland"))
            screenshot(win, out / "other-device.png")
            # Inspect the actual GPS painter without injecting any mock position.
            navigate(win, "GPS")
            wait_for(lambda: locate(win, probe, "Start GPS"))
            time.sleep(3)
            screenshot(win, out / "gps-marker.png")
            pixels = subprocess.check_output([shutil.which("magick") or "convert", str(out / "gps-marker.png"), "-depth", "8", "rgb:-"])
            assert sum(pixels[i:i+3] == bytes((30,115,255)) for i in range(0,len(pixels),3)) > 40
            key(win, "ctrl+q")
            app.wait(timeout=8)
            (out / "result.json").write_text(json.dumps(dict(checks=["passive_open", "timezone", "invalid_date", "manual_datetime", "computer_sync", "automatic_settings", "adb_timeout_navigation_recovery", "switch_device", "blue_gps_marker"]), indent=2))
            print("Date/time utility and GPS marker UI passed")
        finally:
            if app.poll() is None:
                if win:
                    screenshot(win, out / "failure.png")
                    key(win, "ctrl+q")
                try:
                    app.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
