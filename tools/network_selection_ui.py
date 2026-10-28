"""Check Network batch selection against an isolated, seeded local history.

xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/network_selection_ui.py [binary]
Requires xdotool, ImageMagick and Tesseract English data. No Flutter app is touched.
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
out = Path(os.environ.get("INSPECTOR_UI_OUTPUT", str(root / "artifacts/network-selection")))
out.mkdir(parents=True, exist_ok=True)
(out / "failure.png").unlink(missing_ok=True)
probe = out / "probe.png"
env = dict(os.environ)
env.pop("WAYLAND_DISPLAY", None)
env.pop("INSPECTOR_VM_URI", None)

with tempfile.TemporaryDirectory(prefix="inspector-network-selection-") as temporary:
    data = Path(temporary)
    database = data / "inspector.sqlite"
    now = int(time.time() * 1000)

    def insert(db, identifier, started):
        url = "https://selection.test/" + identifier
        exchange = dict(id=identifier, sessionId="fixture", clientId="api", startedAt=started,
            durationMs=12, source="test", request=dict(method="GET", url=url, headers=[], body=None),
            response=dict(status=200, headers=[], body=None), state="complete", error=None, pinned=False)
        db.execute("INSERT INTO exchanges VALUES(?,?,?,?,?)",
            (identifier, started, 0, "get " + url + " api", json.dumps(exchange)))

    with sqlite3.connect(database) as db:
        db.executescript("""CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE exchanges(id TEXT PRIMARY KEY, started INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0, search TEXT NOT NULL, data TEXT NOT NULL);""")
        db.execute("INSERT INTO settings VALUES(1,?)", (json.dumps(dict(
            retentionDays=30, quotaBytes=5 * 1024**3, dark=True, autoObserve=False)),))
        for i in range(205):
            insert(db, f"item{i:03d}", now - i)
    adb = data / "adb"
    adb.write_text("#!/bin/sh\nprintf 'List of devices attached\\n'\n")
    adb.chmod(0o755)
    env.update(INSPECTOR_DATA_DIR=str(data), INSPECTOR_ADB=str(adb),
        XDG_CONFIG_HOME=str(data / "config"))

    def ids():
        with sqlite3.connect(database) as db:
            return {row[0] for row in db.execute("SELECT id FROM exchanges")}

    with (out / "desktop.log").open("w") as log:
        desktop = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        win = None
        try:
            win = wait_for(lambda: window(pid=desktop.pid))
            subprocess.run(["xdotool", "windowsize", win, "1440", "900"], check=True)
            wait_for(lambda: locate(win, probe, "Method"))

            def table():
                method = locate(win, probe, "Method")
                assert method, "Missing history table"
                return method[0] - 50, method[1]

            def row_y(index):
                # Selected rows change foreground/background colors, making URL
                # OCR unreliable. The table uses fixed 29px rows plus 7px spacing.
                return table()[1] + 37 + index * 36

            def check_row(index):
                x, _ = table()
                click(win, x, row_y(index))

            def check_page():
                click(win, *table())

            def selected(count):
                # The button is always visible, including its disabled zero state.
                text = " ".join(w["text"] for w in words(win, probe))
                return f"({count})" in text

            def assert_selected(count):
                wait_for(lambda: selected(count))

            screenshot(win, out / "initial.png")
            # Open an unselected request, then check two other requests.
            click(win, 450, row_y(1))
            wait_for(lambda: locate(win, probe, "Exchange"))
            check_row(0)
            check_row(2)
            assert_selected(2)
            heading = locate(win, probe, "Exchange")
            detail = [w["text"] for w in words(win, probe) if int(w["top"]) >= heading[1]-10]
            assert any("item001" in word for word in detail), detail
            screenshot(win, out / "two-selected.png")
            click_text(win, probe, "Clear selected")
            wait_for(lambda: len(ids()) == 203)
            assert "item000" not in ids() and "item002" not in ids()
            assert "item001" in ids()
            assert locate(win, probe, "Exchange"), "Unselected detail must stay open"
            assert_selected(0)
            click(win, 1410, heading[1])

            # Inserting newer traffic must not move selection to the new row.
            check_row(0)  # item001
            assert_selected(1)
            with sqlite3.connect(database) as db:
                insert(db, "fresh", now + 1)
            click(win, *locate(win, probe, "Refresh", minimum_y=45))
            wait_for(lambda: locate(win, probe, "selection.test/fresh"))
            assert_selected(1)
            click_text(win, probe, "Clear selected")
            wait_for(lambda: "item001" not in ids())
            assert "fresh" in ids() and len(ids()) == 203

            # Header checkbox affects only the current page, not every filtered result.
            check_page()
            assert_selected(200)
            screenshot(win, out / "page-selected.png")
            # Next button follows the count, before selection summary.
            count = locate(win, probe, "of 203")
            assert count
            click(win, table()[0] + 96, count[1])
            wait_for(lambda: locate(win, probe, "selection.test/item202"))
            assert_selected(200)
            check_page()
            assert_selected(203)
            check_page()
            assert_selected(200)
            screenshot(win, out / "selection-outside-page.png")
            click_text(win, probe, "Deselect all")
            assert_selected(0)
            check_page()
            assert_selected(3)

            # Hidden selections remain explicit and clear only their original IDs.
            method = locate(win, probe, "Method")
            enter(win, 500, method[1] - 80, "fresh")
            wait_for(lambda: locate(win, probe, "selection.test/fresh"))
            assert_selected(3)
            screenshot(win, out / "filtered-selection.png")
            click_text(win, probe, "Clear selected")
            wait_for(lambda: len(ids()) == 200)
            assert {"item202", "item203", "item204"}.isdisjoint(ids())
            assert "fresh" in ids()
            assert_selected(0)
            # Disabled clear must be a no-op, never an accidental Clear all.
            click_text(win, probe, "Clear selected")
            time.sleep(0.3)
            assert len(ids()) == 200
            screenshot(win, out / "cleared-selection.png")
            key(win, "ctrl+q")
            desktop.wait(timeout=8)
            print("Network selection passed: independent detail, stable IDs, paging, filtering, batch clear, empty no-op")
        finally:
            if desktop.poll() is None:
                if win:
                    screenshot(win, out / "failure.png")
                    key(win, "ctrl+q")
                try:
                    desktop.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    desktop.kill()
                    desktop.wait()
