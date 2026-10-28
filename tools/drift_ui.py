"""Drive Drift's native grid against isolated, real Flutter fixture databases.

Needs Xvfb, xdotool, ImageMagick, and Tesseract with English language data.
Build target/debug/examples/fixture_query before running.
"""
from pathlib import Path
import json
import csv
import io
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from ui_driver import navigate, window as find_fixture_window

root = Path(__file__).resolve().parents[1]
output = root / "artifacts/drift-ui"
output.mkdir(parents=True, exist_ok=True)
environment = dict(os.environ)
environment.pop("WAYLAND_DISPLAY", None)
languages = subprocess.run(["tesseract", "--list-langs"], capture_output=True,
    text=True, check=True).stdout.splitlines()
if "eng" not in languages:
    raise SystemExit("Install Tesseract English data, or set TESSDATA_PREFIX to its directory.")

def wait_for(callback, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = callback()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError("Timed out waiting for " + callback.__name__)

def click(window, x, y, count=1):
    subprocess.run(["xdotool", "windowraise", window, "windowfocus", window,
                    "mousemove", "--window", window, str(x), str(y),
                    "click", "--repeat", str(count), "--delay", "90", "1"], check=True)
    time.sleep(0.2)

def type_text(window, text):
    subprocess.run(["xdotool", "key", "--window", window, "ctrl+a"], check=True)
    subprocess.run(["xdotool", "type", "--window", window, "--clearmodifiers",
                    "--delay", "1", "--", text], check=True)

def key(window, value):
    subprocess.run(["xdotool", "key", "--window", window, value], check=True)
    time.sleep(0.15)

def screenshot(window, name):
    subprocess.run(["xdotool", "windowraise", window], check=True)
    time.sleep(0.15)
    subprocess.run(["import", "-window", window, str(output / name)], check=True)

def query(uri, sql, database="FixtureDatabase"):
    result = subprocess.run([str(root / "target/debug/examples/fixture_query")],
        input=json.dumps({"uri":uri, "database":database, "sql":sql}),
        capture_output=True, text=True, timeout=20, check=True)
    return json.loads(result.stdout)

def screen_text(window):
    screenshot(window, "screen.png")
    return subprocess.run(["tesseract", str(output / "screen.png"), "stdout", "--psm", "11"],
        capture_output=True, text=True, check=True).stdout

def click_text(window, phrase):
    screenshot(window, "screen.png")
    result = subprocess.run(["tesseract", str(output / "screen.png"), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1"],
        capture_output=True, text=True, check=True)
    lines = {}
    for word in csv.DictReader(io.StringIO(result.stdout), delimiter="\t"):
        if not word["text"].strip():
            continue
        lines.setdefault((word["block_num"],word["par_num"],word["line_num"]),[]).append(word)
    needle = phrase.lower().split()
    for words in lines.values():
        tokens = [w["text"].lower() for w in words]
        for i in range(len(tokens)-len(needle)+1):
            if tokens[i:i+len(needle)] == needle:
                hit = words[i:i+len(needle)]
                x = min(int(w["left"]) for w in hit)
                y = min(int(w["top"]) for w in hit)
                right = max(int(w["left"])+int(w["width"]) for w in hit)
                bottom = max(int(w["top"])+int(w["height"]) for w in hit)
                click(window,(x+right)//2,(y+bottom)//2)
                return
    raise AssertionError("UI text not found: " + phrase)

def select_database(window, audit):
    click(window, 295, 107)
    click_text(window, "AuditDatabase" if audit else "FixtureDatabase")
    time.sleep(0.5)

def scenario(window, uri):
    screenshot(window, "tables-and-grid.png")
    if os.environ.get("DRIFT_UI_LAYOUT_ONLY") == "1":
        return
    # Edit two cells in one transaction directly in the grid.
    click(window, 680, 230, count=2)
    type_text(window, "Edited from grid")
    key(window, "Return")
    click(window, 900, 230, count=2)
    type_text(window, "1")
    key(window, "Return")
    screenshot(window, "pending-edits.png")
    click(window, 565, 96)
    wait_for(lambda: query(uri,"SELECT title,completed FROM notes WHERE id=1") == [{"title":"Edited from grid","completed":1}])
    screenshot(window, "saved-edits.png")
    click(window, 465, 96)
    time.sleep(0.3)
    type_text(window, "Inserted from grid")
    key(window, "Return")
    click(window, 565, 96)
    wait_for(lambda: query(uri,"SELECT count(*) AS n FROM notes")[0]["n"] == 3)
    assert query(uri,"SELECT completed FROM notes WHERE title='Inserted from grid'") == [{"completed":0}]
    screenshot(window, "inserted-row.png")
    click(window, 433, 230)
    click(window, 770, 96)
    screenshot(window, "delete-confirmation.png")
    click(window, 615, 491)
    wait_for(lambda: query(uri,"SELECT count(*) AS n FROM notes")[0]["n"] == 2)
    assert query(uri,"SELECT id FROM notes WHERE id=1") == []
    # Empty is explicitly confirmed and leaves the schema usable.
    click(window, 890, 96)
    click(window, 535, 500)
    assert query(uri,"SELECT count(*) AS n FROM notes")[0]["n"] == 2
    click(window, 890, 96)
    screenshot(window, "empty-confirmation.png")
    click(window, 620, 500)
    wait_for(lambda: query(uri,"SELECT count(*) AS n FROM notes")[0]["n"] == 0)
    screenshot(window, "empty-table.png")
    select_database(window, True)
    text = screen_text(window)
    assert "audit_log" in text and "Audit database only" in text, text
    screenshot(window,"audit-database.png")
    select_database(window, False)
    assert "notes" in screen_text(window)
    # A 12.5 MiB page must still render bounded previews and switch cleanly.
    click(window,1375,62)
    click(window,680,112)
    type_text(window,"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<200) INSERT INTO notes(id,title,completed) SELECT x,hex(zeroblob(32768)),0 FROM n")
    click_text(window,"Execute statement")
    wait_for(lambda: query(uri,"SELECT count(*) AS n FROM notes")[0]["n"] == 200)
    time.sleep(1)
    click(window,1375,62)
    time.sleep(1)
    screenshot(window,"large-cells.png")
    subprocess.run(["xdotool","mousemove","--window",window,"900","500","click","--repeat","12","--delay","35","5"],check=True)
    screenshot(window,"large-cells-scrolled.png")
    for _ in range(3):
        select_database(window,True)
        select_database(window,False)
    select_database(window,True)
    assert "audit_log" in screen_text(window)
    assert query(uri,"SELECT count(*) AS n FROM audit_log","AuditDatabase")[0]["n"] == 1
    # Open a third database after the Inspector is already connected. Drift's
    # lifecycle event must refresh the picker without reconnecting.
    fixture_window = wait_for(lambda: find_fixture_window(name="^inspector_fixture$"))
    click_text(fixture_window, "Open late database")
    time.sleep(0.8)
    click(window, 295, 107)
    wait_for(lambda: "LateDatabase" in screen_text(window))
    click_text(window, "LateDatabase")
    wait_for(lambda: "Opened after connect" in screen_text(window))
    screenshot(window,"late-database-opened.png")
    click_text(fixture_window, "Close late database")
    wait_for(lambda: "LateDatabase" not in screen_text(window) and "notes" in screen_text(window))
    click(window, 295, 107)
    assert "LateDatabase" not in screen_text(window)
    key(window, "Escape")
    screenshot(window,"late-database-closed.png")
    (output/"result.json").write_text(json.dumps({"inline_batch_update":True,"insert_with_defaults":True,"delete_row":True,"empty_with_confirmation":True,"database_switch":True,"late_database_discovery":True,"closed_database_removed":True,"large_page_bytes":200*65536},indent=2))

with tempfile.TemporaryDirectory(prefix="inspector-drift-ui-") as temporary:
    data = Path(temporary)
    environment["XDG_CONFIG_HOME"] = str(data / "config")
    fixture_log = (output / "fixture.log").open("w")
    desktop_log = (output / "desktop.log").open("w")
    fixture = subprocess.Popen(["flutter", "run", "--no-pub", "-d", "linux", "--debug",
        "--dart-define=INSPECTOR_BRIDGE=false"], cwd=root / "examples/flutter_fixture",
        env=environment, stdout=fixture_log, stderr=fixture_log, stdin=subprocess.PIPE,
        start_new_session=True)
    desktop = None
    window = None
    try:
        def service():
            match = re.search(r"http://127\.0\.0\.1:\d+/[A-Za-z0-9_=\-]+/",
                (output / "fixture.log").read_text())
            return match.group(0) if match else None
        uri = wait_for(service, 120)
        wait_for(lambda: query(uri, "SELECT count(*) AS n FROM notes")[0]["n"] == 2)
        binary = Path(sys.argv[1]).resolve() if len(sys.argv)>1 else root / "target/debug/flutter-inspector"
        desktop = subprocess.Popen([str(binary)], env={**environment,
            "INSPECTOR_VM_URI":uri, "INSPECTOR_DATA_DIR":str(data / "inspector"),
            "INSPECTOR_FRAME_LOG":str(output / "frames.json")},
            stdout=desktop_log, stderr=desktop_log)
        def find_window():
            result = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid",
                str(desktop.pid)], capture_output=True, text=True)
            return result.stdout.splitlines()[0] if result.returncode == 0 else None
        window = wait_for(find_window)
        time.sleep(2)
        navigate(window, "Drift")
        time.sleep(1)
        scenario(window, uri)
        key(window, "ctrl+q")
        desktop.wait(timeout=8)
        print("Drift UI completed; frame timings:", (output / "frames.json").read_text())
    except Exception:
        if window is not None and desktop.poll() is None:
            screenshot(window, "failure.png")
        raise
    finally:
        if desktop is not None and desktop.poll() is None:
            desktop.terminate()
            desktop.wait(timeout=5)
        if fixture.poll() is None:
            os.killpg(fixture.pid, signal.SIGTERM)
            fixture.wait(timeout=5)
        fixture_log.close()
        desktop_log.close()
