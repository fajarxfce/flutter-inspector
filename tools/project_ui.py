"""Real Flutter launcher + colored Logcat + Network clear integration, isolated data.

xvfb-run -a -s '-screen 0 1600x1100x24' python3 tools/project_ui.py [binary]
Requires Flutter, xdotool, ImageMagick and Tesseract English data.
"""
from pathlib import Path
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import click, click_text, enter, key, locate, screenshot, wait_for, window, words

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/debug/flutter-inspector"
out = Path(os.environ.get("INSPECTOR_UI_OUTPUT", str(root / "artifacts/project-ui")))
out.mkdir(parents=True, exist_ok=True)
probe = out / "probe.png"
env = dict(os.environ)
env.pop("WAYLAND_DISPLAY", None)
env.pop("INSPECTOR_VM_URI", None)

with tempfile.TemporaryDirectory(prefix="inspector-project-ui-") as directory:
    data = Path(directory)
    control = data / "log-control.json"
    control.write_text(json.dumps({"enabled": True, "marker": "INITIAL"}))
    adb = data / "adb"
    adb.write_text(f'''#!{sys.executable}
import json, sys, time
from pathlib import Path
args = sys.argv[1:]
if args[:1] == ['devices']:
    print('List of devices attached\\nfake-phone device model:Test_Phone')
elif 'logcat' in args and '-d' not in args:
    while True:
        control = json.loads(Path({str(control)!r}).read_text())
        if control['enabled']:
            for pid in [4242, 77]:
                for level in ['V', 'D', 'I', 'W', 'E', 'F']:
                    owner = 'CURRENT_APP' if pid == 4242 else 'OTHER_APP'
                    print(f"10-02 12:34:56.123 {{pid}} {{pid}} {{level}} FlutterTest: {{control['marker']}} {{owner}} level={{level}} pid={{pid}}", flush=True)
        time.sleep(0.3)
''')
    adb.chmod(0o755)
    env.update(INSPECTOR_DATA_DIR=str(data), INSPECTOR_ADB=str(adb),
        INSPECTOR_FRAME_LOG=str(out / "frames.json"), XDG_CONFIG_HOME=str(data / "config"))
    log = (out / "desktop.log").open("w")
    desktop = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
    win = None
    try:
        win = wait_for(lambda: window(pid=desktop.pid))
        subprocess.run(["xdotool", "windowsize", win, "1440", "900"], check=True)
        # Isolate launcher from unrelated debug sessions on the developer machine.
        click(win, 406, 15)  # disable Auto observe before opening our own project
        wait_for(lambda: locate(win, probe, "Streaming"))
        pid_point = locate(win, probe, "App PID")
        assert pid_point, "Missing App PID override"
        enter(win, *pid_point, "4242")
        time.sleep(0.8)
        screenshot(win, out / "logcat-mine.png")
        mine = " ".join(w["text"] for w in words(win, probe))
        normalized = "".join(c for c in mine.upper() if c.isalpha())
        assert "CURRENTAPP" in normalized and "OTHERAPP" not in normalized, mine
        click_text(win, probe, "Semua log")
        time.sleep(0.4)
        screenshot(win, out / "logcat-all.png")
        wait_for(lambda: "OTHERAPP" in "".join(c for w in words(win, probe) for c in w["text"].upper() if c.isalpha()))
        # Drag across visible log rows, copy with Ctrl+C, and retain selection as new logs arrive.
        selection_y = locate(win, probe, "FlutterTest", minimum_y=735)[1]
        subprocess.run(["xdotool", "windowfocus", win, "mousemove", "--window", win, "405", str(selection_y),
            "mousedown", "1", "sleep", "0.25", "mousemove", "--window", win, "950", str(selection_y + 45),
            "sleep", "0.25", "mouseup", "1"], check=True)
        time.sleep(0.2)
        key(win, "ctrl+c")
        time.sleep(0.3)
        copied = subprocess.run(["xclip", "-selection", "clipboard", "-o"], capture_output=True, text=True, check=True).stdout
        assert "\n" in copied and "FlutterTest" in copied, repr(copied)
        time.sleep(1)
        time.sleep(0.2)
        key(win, "ctrl+c")
        time.sleep(0.3)
        copied_again = subprocess.run(["xclip", "-selection", "clipboard", "-o"], capture_output=True, text=True, check=True).stdout
        assert copied_again == copied, "Selection moved while new logs arrived"
        screenshot(win, out / "logcat-manual-copy.png")
        # Clear with the producer paused; then new lines must continue to arrive.
        control.write_text(json.dumps({"enabled": False, "marker": "INITIAL"}))
        time.sleep(0.5)
        click(win, *locate(win, probe, "Clear", minimum_y=600))
        wait_for(lambda: locate(win, probe, "0 / 0 lines"))
        control.write_text(json.dumps({"enabled": True, "marker": "AFTER_CLEAR"}))
        wait_for(lambda: locate(win, probe, "AFTER_CLEAR"))
        screenshot(win, out / "logcat-after-clear.png")
        # Drag the top edge of the dock and resize the native window.
        title = locate(win, probe, "Logcat")
        subprocess.run(["xdotool", "mousemove", "--window", win, "700", str(title[1] - 11), "mousedown", "1", "sleep", "0.25", "mousemove", "--window", win, "700", str(title[1] - 95), "sleep", "0.25", "mouseup", "1"], check=True)
        time.sleep(0.3)
        assert abs(locate(win, probe, "Logcat")[1] - title[1]) > 30, "Dock did not resize"
        screenshot(win, out / "logcat-resized.png")
        subprocess.run(["xdotool", "windowsize", win, "1100", "750"], check=True)
        screenshot(win, out / "window-resized.png")
        assert locate(win, probe, "Logcat"), "Dock disappeared after resizing"
        subprocess.run(["xdotool", "windowsize", win, "1440", "900"], check=True)
        # Restore the dock to ~220px so the launch form has room.
        title = locate(win, probe, "Logcat")
        subprocess.run(["xdotool", "mousemove", "--window", win, "700", str(title[1] - 11), "mousedown", "1", "sleep", "0.25", "mousemove", "--window", win, "700", "655", "sleep", "0.25", "mouseup", "1"], check=True)
        if os.environ.get("INSPECTOR_TEST_LOGCAT_ONLY") == "1":
            key(win, "ctrl+q")
            desktop.wait(timeout=8)
            print("Logcat manual multi-line copy, streaming, clear, resize passed")
            sys.exit(0)
        click_text(win, probe, "Projects")
        folder = root / "examples/flutter_fixture"
        enter(win, *locate(win, probe, "Project folder"), str(folder))
        click_text(win, probe, "Open project")
        wait_for(lambda: locate(win, probe, "Default debug"))
        screenshot(win, out / "project-opened.png")
        # Choose Linux explicitly; never launch the user's Android application.
        device_label = locate(win, probe, "Device")
        click(win, device_label[0] + 100, device_label[1])
        click_text(win, probe, "linux")
        click_text(win, probe, "Launch details")
        screenshot(win, out / "launch-details.png")
        # The argument editor is below the working-directory/description labels.
        hint = locate(win, probe, "Working directory")
        enter(win, hint[0] + 130, hint[1] + 60, "--no-pub\n--dart-define=INSPECTOR_BRIDGE=false")
        click_text(win, probe, "Launch details")
        click_text(win, probe, "Run debug")
        wait_for(lambda: locate(win, probe, "Running in debug mode"), seconds=120)
        fixture = wait_for(lambda: window(name="^inspector_fixture$"), seconds=120)
        wait_for(lambda: locate(win, probe, "Connected"), seconds=30)
        screenshot(win, out / "project-running.png")
        def rows():
            with sqlite3.connect(data / "inspector.sqlite") as db:
                return [json.loads(r[0]) for r in db.execute("SELECT data FROM exchanges")]
        click(fixture, 90, 177)
        wait_for(lambda: [r for r in rows() if r["state"] == "complete"])
        first_session = rows()[0]["sessionId"]
        click_text(win, probe, "Hot reload")
        wait_for(lambda: locate(win, probe, "Running in debug mode"), seconds=40)
        click_text(win, probe, "Hot restart")
        wait_for(lambda: locate(win, probe, "Running in debug mode"), seconds=40)
        wait_for(lambda: locate(win, probe, "Connected"), seconds=30)
        def restarted_capture():
            click(fixture, 90, 177)
            return [r for r in rows() if r["state"] == "complete" and r["sessionId"] != first_session]
        wait_for(restarted_capture, seconds=30)
        screenshot(win, out / "project-restarted.png")
        click_text(win, probe, "Network")
        click(win, 400, 215)
        subprocess.run(["xdotool", "click", "3"], check=True)
        click_text(win, probe, "Send to Repeater")
        time.sleep(1)
        click_text(win, probe, "Network")
        count = len(rows())
        click(win, 400, 215)
        subprocess.run(["xdotool", "click", "3"], check=True)
        click_text(win, probe, "Clear item")
        wait_for(lambda: len(rows()) == count - 1)
        screenshot(win, out / "network-clear-item.png")
        # Filter hides every row; Clear all must still operate on all history.
        enter(win, 330, 92, "no-match-at-all")
        click(win, 490, 61)
        screenshot(win, out / "network-clear-confirm.png")
        key(win, "Escape")
        time.sleep(0.2)
        assert len(rows()) == count - 1
        click(win, 490, 61)
        click(win, 617, 507)
        wait_for(lambda: len(rows()) == 0)
        time.sleep(1)
        assert len(rows()) == 0, "Cleared requests came back"
        with sqlite3.connect(data / "inspector.sqlite") as db:
            assert db.execute("SELECT COUNT(*) FROM drafts").fetchone()[0] > 0
            assert db.execute("SELECT COUNT(*) FROM bodies").fetchone()[0] == 0
        enter(win, 330, 92, "")
        click(fixture, 90, 177)
        wait_for(lambda: len(rows()) > 0)
        screenshot(win, out / "network-new-after-clear.png")
        click_text(win, probe, "Projects")
        click_text(win, probe, "Stop")
        wait_for(lambda: locate(win, probe, "Stopped"), seconds=20)
        wait_for(lambda: not window(name="^inspector_fixture$"))
        screenshot(win, out / "project-stopped.png")
        with sqlite3.connect(data / "inspector.sqlite") as db:
            settings = json.loads(db.execute("SELECT data FROM settings").fetchone()[0])
            assert str(folder) in settings["recentProjects"]
        key(win, "ctrl+q")
        desktop.wait(timeout=8)
        (out / "result.json").write_text(json.dumps({"logcat": ["mine", "all", "colors", "clear", "continued_stream", "resizable_dock", "manual_multiline_copy", "stable_selection_while_streaming"],
            "launcher": ["open", "device", "run_debug", "auto_connect", "hot_reload", "hot_restart_reconnect", "stop", "recent_persisted"],
            "network": ["clear_item", "clear_all_with_filter", "cancel", "preserve_draft", "body_cleanup", "new_capture"]}, indent=2))
        print("Launcher, Logcat and Network clear UI passed")
    except Exception:
        if win and desktop.poll() is None:
            screenshot(win, out / "failure.png")
        raise
    finally:
        if desktop.poll() is None:
            if win:
                key(win, "ctrl+q")
            try:
                desktop.wait(timeout=8)
            except subprocess.TimeoutExpired:
                desktop.kill()
                desktop.wait()
        log.close()
