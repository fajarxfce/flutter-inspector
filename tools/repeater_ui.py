"""Exercise Network → multi-tab Repeater with isolated history and a local HTTP server.

xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/repeater_ui.py [binary]
"""
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import click, click_text, enter, key, locate, navigate, screenshot, wait_for, window, words

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/debug/flutter-inspector"
out = Path(os.environ.get("INSPECTOR_UI_OUTPUT", root / "artifacts/repeater-ui"))
out.mkdir(parents=True, exist_ok=True)
(out / "failure.png").unlink(missing_ok=True)
probe = out / "probe.png"
received = []
in_flight = set()
overlap = []


class Echo(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", "0"))).decode()
        received.append((self.path, body, self.headers.get("X-Edit")))
        if in_flight:
            overlap.append(self.path)
        in_flight.add(self.path)
        time.sleep(1 if "alpha" in self.path else 6)
        payload = json.dumps(dict(path=self.path, body=body, ok=True, amount=9007199254740993)).encode()
        self.send_response(201 if "alpha" in self.path else 202)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("X-Repeat", "one")
        self.send_header("X-Repeat", "two")
        self.end_headers()
        self.wfile.write(payload)
        in_flight.remove(self.path)

    def log_message(self, *_):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Echo)
Thread(target=server.serve_forever, daemon=True).start()
env = dict(os.environ)
env.pop("WAYLAND_DISPLAY", None)
env.pop("INSPECTOR_VM_URI", None)

with tempfile.TemporaryDirectory(prefix="inspector-repeater-") as temporary:
    data = Path(temporary)
    database = data / "inspector.sqlite"
    (data / "bodies").mkdir()
    originals = {}
    with sqlite3.connect(database) as db:
        db.executescript("""PRAGMA user_version=2;
            CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE exchanges(id TEXT PRIMARY KEY, started INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0, search TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE bodies(id TEXT PRIMARY KEY, size INTEGER NOT NULL);
            CREATE TABLE exchange_bodies(exchange_id TEXT NOT NULL REFERENCES exchanges(id) ON DELETE CASCADE,
                body_id TEXT NOT NULL, PRIMARY KEY(exchange_id, body_id));""")
        db.execute("INSERT INTO settings VALUES(1,?)", (json.dumps(dict(
            retentionDays=30, quotaBytes=5 * 1024**3, dark=True, autoObserve=False)),))
        for index, name in enumerate(("alpha", "beta")):
            descriptors = []
            for side, body in (("request", dict(message="original-" + name)),
                               ("response", dict(captured=name, ok=True, amount=9007199254740993, empty=None))):
                identifier = name + "-" + side
                raw = json.dumps(body).encode()
                (data / "bodies" / hashlib.sha256(identifier.encode()).hexdigest()).write_bytes(raw)
                db.execute("INSERT INTO bodies VALUES(?,?)", (identifier, len(raw)))
                db.execute("INSERT INTO exchange_bodies VALUES(?,?)", (name, identifier))
                descriptors.append(dict(id=identifier, size=len(raw), complete=True, contentType="application/json"))
            url = f"http://127.0.0.1:{server.server_port}/{name}?q=a+b&q=%2B&empty="
            exchange = dict(id=name, sessionId="fixture", clientId="api", startedAt=int(time.time()*1000)-index,
                durationMs=17, source="test", state="complete", pinned=False, error=None,
                request=dict(method="POST", url=url, body=descriptors[0], headers=[
                    dict(name="Content-Type", value="application/json"), dict(name="X-Edit", value="original")]),
                response=dict(status=200, body=descriptors[1], headers=[
                    dict(name="Content-Type", value="application/json"), dict(name="X-Repeat", value="one"), dict(name="X-Repeat", value="two")]))
            originals[name] = exchange
            db.execute("INSERT INTO exchanges VALUES(?,?,?,?,?)", (name, exchange["startedAt"], 0, url, json.dumps(exchange)))
    adb = data / "adb"
    adb.write_text("#!/bin/sh\nprintf 'List of devices attached\\n'\n")
    adb.chmod(0o755)
    env.update(INSPECTOR_DATA_DIR=str(data), INSPECTOR_ADB=str(adb), XDG_CONFIG_HOME=str(data / "config"))

    def query(sql):
        with sqlite3.connect(database) as db:
            return db.execute(sql).fetchall()

    def result(name):
        for draft, exchange in query("SELECT d.data,r.data FROM drafts d JOIN repeater_results r ON d.id=r.draft_id"):
            if json.loads(draft)["name"] == name:
                value = json.loads(exchange)
                if value["state"] == "complete":
                    return value

    def move_away(win):
        subprocess.run(["xdotool", "mousemove", "--window", win, "180", "840"], check=True)
        time.sleep(0.15)

    def right_click(win, x, y):
        subprocess.run(["xdotool", "windowraise", win, "windowfocus", win,
            "mousemove", "--window", win, str(x), str(y), "click", "3"], check=True)
        time.sleep(0.2)

    def send_from_menu(win, row_y):
        # Target the first action in the actual row popup.
        screenshot(win, out / "context-menu.png")
        click(win, 470, row_y + 17)

    log = (out / "desktop.log").open("w")
    desktop = None
    win = None
    try:
        desktop = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        win = wait_for(lambda: window(pid=desktop.pid))
        subprocess.run(["xdotool", "windowsize", win, "1440", "900"], check=True)
        wait_for(lambda: locate(win, probe, "Duration"))
        assert locate(win, probe, "Time")
        screenshot(win, out / "network-time.png")
        # Response JSON is pretty and colored by default, retaining large integers.
        method = locate(win, probe, "Method")
        first_y = method[1] + 37
        click(win, 450, first_y)
        wait_for(lambda: locate(win, probe, "9007199254740993"))
        screenshot(win, out / "network-json.png")
        pixels = subprocess.check_output([shutil.which("magick") or "convert", str(out / "network-json.png"), "-crop", "1240x350+180+300", "-depth", "8", "rgb:-"])
        colors = [tuple(pixels[i:i+3]) for i in range(0, len(pixels), 3)]
        for color in ((113, 190, 235), (151, 210, 160), (239, 181, 112), (199, 151, 238)):
            assert sum(pixel == color for pixel in colors) > 5, ("Missing JSON syntax color", color)
        labels = words(win, probe)
        assert any(w["text"] == "Request" and int(w["top"]) > 300 for w in labels)
        assert any(w["text"] == "Response" and int(w["top"]) > 300 for w in labels)
        assert not any(w["text"].lower() == "copy" and 250 < int(w["top"]) < 650 for w in labels), "Detail actions must live in context menus"
        # Copy response from the detail context menu, retaining headers and exact bytes.
        response_heading = locate(win, probe, "Response", minimum_y=300)
        right_click(win, *response_heading)
        click_text(win, probe, "Copy response")
        copied = subprocess.check_output(["xclip", "-selection", "clipboard", "-o"], text=True)
        assert copied.startswith("HTTP/1.1 200") and "X-Repeat: one" in copied and "9007199254740993" in copied
        # The body remains manually selectable even with its surrounding context menu.
        captured = locate(win, probe, "captured", minimum_y=300)
        assert captured
        subprocess.run(["xclip", "-selection", "clipboard"], input="selection-sentinel", text=True, check=True)
        subprocess.run(["xdotool", "mousemove", "--window", win, str(captured[0]-40), str(captured[1]), "mousedown", "1"], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "mousemove", "--window", win, str(captured[0]+45), str(captured[1])], check=True)
        time.sleep(0.2)
        subprocess.run(["xdotool", "mouseup", "1"], check=True)
        key(win, "ctrl+c")
        time.sleep(0.2)
        assert "captured" in subprocess.check_output(["xclip", "-selection", "clipboard", "-o"], text=True)
        # Choose the Response's Headers subview while Request stays visible.
        headers = next(w for w in words(win, probe) if w["text"] == "Headers" and int(w["left"]) > 800 and int(w["top"]) > 300)
        click(win, int(headers["left"])+10, int(headers["top"])+6)
        wait_for(lambda: locate(win, probe, "X-Repeat"))
        screenshot(win, out / "response-headers.png")
        click_text(win, probe, "Parameters")
        wait_for(lambda: locate(win, probe, "empty"))
        screenshot(win, out / "request-parameters.png")
        # The red close control belongs to the upper right of the detail, and
        # closing it returns the whole work area to the history table.
        heading = locate(win, probe, "Exchange", minimum_y=250)
        red = subprocess.check_output([shutil.which("magick") or "convert", str(out / "request-parameters.png"),
            "-crop", f"40x40+1390+{heading[1]-20}", "-depth", "8", "rgb:-"])
        assert sum(red[i] > 180 and red[i] > red[i+1]*1.5 and red[i+2] < 170 for i in range(0,len(red),3)) > 5
        click(win, 1410, heading[1])
        # The status bar still says "2 exchanges" after closing the detail.
        wait_for(lambda: not any(w["text"] == "Exchange" for w in words(win, probe)))
        click(win, 450, first_y)
        wait_for(lambda: locate(win, probe, "Exchange"))
        # The right-click action must use its own row, independent of the open detail.
        right_click(win, 420, first_y)
        send_from_menu(win, first_y)
        wait_for(lambda: locate(win, probe, "Follow redirects"))
        name = locate(win, probe, "Name")
        enter(win, name[0] + 90, name[1], "Alpha")
        move_away(win)
        screenshot(win, out / "repeater-request.png")
        navigate(win, "Network")
        right_click(win, 420, first_y + 36)
        send_from_menu(win, first_y + 36)
        wait_for(lambda: locate(win, probe, "Follow redirects"))
        name = locate(win, probe, "Name")
        enter(win, name[0] + 90, name[1], "Beta")
        time.sleep(0.8)
        assert len(query("SELECT id FROM drafts")) == 2
        beta = next(json.loads(row[0]) for row in query("SELECT data FROM drafts") if json.loads(row[0])["name"] == "Beta")
        assert "/beta?" in beta["url"], "Context action must use its row, not the selected detail"
        # Edit only Beta's request body and headers. Alpha keeps its original bytes.
        request = locate(win, probe, "Request", minimum_y=160)
        enter(win, request[0] + 100, request[1] + 113, '{"message":"edited-beta"}')
        click_text(win, probe, "Headers")
        edit_header = wait_for(lambda: locate(win, probe, "X-Edit") or locate(win, probe, "XEdit"))
        enter(win, edit_header[0] + 350, edit_header[1], "edited-header")
        click_text(win, probe, "Body")
        move_away(win)
        click(win, *locate(win, probe, "Send", minimum_y=140))
        click_text(win, probe, "Alpha")
        click(win, *locate(win, probe, "Send", minimum_y=140))
        wait_for(lambda: result("Alpha") and result("Beta"))
        wait_for(lambda: locate(win, probe, "Status 201"))
        screenshot(win, out / "alpha-response.png")
        click_text(win, probe, "Beta")
        wait_for(lambda: locate(win, probe, "Status 202"))
        wait_for(lambda: locate(win, probe, "9007199254740993"))
        screenshot(win, out / "beta-response.png")
        assert result("Alpha")["id"] != result("Beta")["id"]
        assert overlap, "Requests must be able to run concurrently across tabs"
        assert {json.loads(row[0])["id"]: json.loads(row[0]) for row in query("SELECT data FROM exchanges")} == originals
        assert any("beta" in path and "edited-beta" in body and header == "edited-header" for path, body, header in received), received
        # Clear all original history. Both saved Repeater responses still work.
        navigate(win, "Network")
        click_text(win, probe, "Clear all")
        click_text(win, probe, "Clear history")
        wait_for(lambda: not query("SELECT id FROM exchanges"))
        navigate(win, "Repeater")
        assert locate(win, probe, "Status 202")
        screenshot(win, out / "after-network-clear.png")
        key(win, "ctrl+q")
        desktop.wait(timeout=8)
        desktop = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        win = wait_for(lambda: window(pid=desktop.pid))
        navigate(win, "Repeater")
        click_text(win, probe, "Saved requests")
        click_text(win, probe, "Beta")
        wait_for(lambda: locate(win, probe, "Status 202"))
        wait_for(lambda: locate(win, probe, "9007199254740993"))
        screenshot(win, out / "saved-response-after-restart.png")
        key(win, "ctrl+q")
        desktop.wait(timeout=8)
        (out / "result.json").write_text(json.dumps(dict(checks=["time_and_duration", "pretty_syntax_colors",
            "headers_and_parameters", "side_by_side_details", "context_only_actions", "red_close_icon", "copy_response", "manual_body_selection", "context_menu", "two_tabs", "editable_headers_body", "independent_responses",
            "originals_unchanged", "clear_network_preserves_results", "restore_after_restart"], requests=received), indent=2))
        print("Network and multi-tab Repeater UI passed")
    finally:
        if desktop and desktop.poll() is None:
            if win:
                screenshot(win, out / "failure.png")
                key(win, "ctrl+q")
            try:
                desktop.wait(timeout=8)
            except subprocess.TimeoutExpired:
                desktop.kill()
                desktop.wait()
        log.close()
        server.shutdown()
