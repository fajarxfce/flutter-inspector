"""Drive the real Flutter fixture and native Inspector under an X11 display."""
from pathlib import Path
import csv
import io
import json
import os
import re
import signal
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import navigate, click_text, locate

root=Path(__file__).resolve().parents[1]
native = os.environ.get("INSPECTOR_TEST_NATIVE") == "1"
observe_check = os.environ.get("INSPECTOR_TEST_OBSERVE") == "1"
output=root/("artifacts/auto-observe-ui" if observe_check else "artifacts/native-ui" if native else "artifacts/integration-ui")
output.mkdir(parents=True,exist_ok=True)
environment=dict(os.environ)
environment.pop("WAYLAND_DISPLAY",None)

def wait_for(callback, seconds=15):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        value=callback()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError("Timed out waiting for "+callback.__name__)

def window(process, fixture=False):
    def find():
        if process.poll() is not None:
            raise RuntimeError("Process exited: "+str(process.returncode))
        query = ["--name", "^inspector_fixture$"] if fixture and native else ["--pid", str(process.pid)]
        result=subprocess.run(["xdotool","search","--onlyvisible",*query],capture_output=True,text=True)
        return result.stdout.splitlines()[0] if result.returncode==0 else None
    return wait_for(find, seconds=120 if fixture and native else 15)

def click(window,x,y):
    subprocess.run(["xdotool","windowraise",window,"mousemove","--window",window,str(x),str(y),"click","1"],check=True)
    time.sleep(0.25)

def screenshot(window,name):
    subprocess.run(["xdotool","windowraise",window],check=True)
    time.sleep(0.3)
    subprocess.run(["import","-window",window,str(output/name)],check=True)

def select_fixture(window):
    # Optional for developer machines with another live Flutter debug session.
    click(window,270,15)
    screenshot(window,"app-selection.png")
    result=subprocess.run(["tesseract",str(output/"app-selection.png"),"stdout",
        "--psm","11","-c","tessedit_create_tsv=1"],capture_output=True,text=True,check=True)
    for word in csv.DictReader(io.StringIO(result.stdout),delimiter="\t",quoting=csv.QUOTE_NONE):
        if "inspector_fixture" in word["text"].lower():
            click(window,int(word["left"])+int(word["width"])//2,
                int(word["top"])+int(word["height"])//2)
            return
    raise RuntimeError("Fixture not found in the auto-discovered app menu")

with tempfile.TemporaryDirectory(prefix="inspector-ui-") as temporary:
    data=Path(temporary)
    environment["XDG_CONFIG_HOME"]=str(data/"config")
    if observe_check:
        # Observe full scans with slow, isolated ADB; IDE/DDS discovery stays real.
        adb_log=output/"adb-calls.jsonl"
        adb_log.write_text("")
        adb=data/"adb"
        adb.write_text(f'''#!{sys.executable}
import json, sys, time
with open({str(adb_log)!r}, 'a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[1:2] == ['devices']:
    time.sleep(2)
    print('List of devices attached')
''')
        adb.chmod(0o755)
        environment["INSPECTOR_ADB"]=str(adb)
        environment["INSPECTOR_FRAME_LOG"]=str(output/"frames.json")
        def discovery_scans():
            return sum(json.loads(line)[0] == "devices" for line in adb_log.read_text().splitlines())
    fixture_log=(output/"fixture.log").open("w")
    desktop_log=(output/"desktop.log").open("w")
    def launch_fixture():
        command = ["flutter","run","--no-pub","-d","linux","--debug","--dart-define=INSPECTOR_BRIDGE=false"] if native else [str(root/"examples/flutter_fixture/build/linux/x64/debug/bundle/inspector_fixture")]
        return subprocess.Popen(command,cwd=root/"examples/flutter_fixture",env=environment,stdin=subprocess.PIPE,stdout=fixture_log,stderr=fixture_log,start_new_session=True)
    fixture=launch_fixture()
    desktop=None
    try:
        fixture_window=window(fixture, fixture=True)
        def service_uri():
            text=(output/"fixture.log").read_text()
            match=re.search(r"http://127\.0\.0\.1:\d+/[A-Za-z0-9_=\-]+/",text)
            return match.group(0) if match else None
        uri=wait_for(service_uri)
        screenshot(fixture_window,"fixture.png")
        desktop_environment={**environment,"INSPECTOR_DATA_DIR":str(data)}
        desktop_environment.pop("INSPECTOR_VM_URI",None)
        desktop_binary=Path(sys.argv[1]).resolve() if len(sys.argv)>1 else root/"target/release/flutter-inspector"
        desktop=subprocess.Popen([str(desktop_binary)],env=desktop_environment,stdout=desktop_log,stderr=desktop_log)
        desktop_window=window(desktop)
        if observe_check:
            subprocess.run(["xdotool","mousemove","--window",desktop_window,"900","600"],check=True)
            screenshot(desktop_window,"automatic-scan-toolbar.png")
        time.sleep(4)
        if observe_check:
            screenshot(desktop_window,"settled-toolbar.png")
            def refresh_pixels(name):
                return subprocess.check_output([shutil.which("magick") or "convert",str(output/name),"-crop","82x28+503+1","-depth","8","rgb:-"])
            assert refresh_pixels("automatic-scan-toolbar.png")==refresh_pixels("settled-toolbar.png"),"Automatic discovery changed or shifted the Refresh control"
        if os.environ.get("INSPECTOR_TEST_PICK_FIXTURE") == "1":
            select_fixture(desktop_window)
        click(fixture_window,90,177)
        def captured():
            try:
                with sqlite3.connect(data/"inspector.sqlite") as db:
                    rows=db.execute("SELECT data FROM exchanges").fetchall()
                    complete=[json.loads(row[0]) for row in rows if json.loads(row[0]).get("state")=="complete"]
                    return complete
            except sqlite3.OperationalError:
                return None
        last_initial_request=0
        def initial_capture():
            global last_initial_request
            rows=captured()
            if rows:
                return rows
            # Discovery and VM evaluation can finish after the first request.
            if time.monotonic()-last_initial_request>=1:
                click(fixture_window,90,177)
                last_initial_request=time.monotonic()
            return None
        rows=wait_for(initial_capture,seconds=30)
        time.sleep(1)
        click(desktop_window,400,215)
        time.sleep(0.7)
        screenshot(desktop_window,"network-json.png")
        subprocess.run(["xdotool", "mousemove", "--window", desktop_window, "400", "215", "click", "3"], check=True)
        click_text(desktop_window, output/"probe.png", "Send to Repeater")
        screenshot(desktop_window,"repeater.png")
        original_rows = {row["id"]: row for row in captured()}
        click(desktop_window, *locate(desktop_window, output/"probe.png", "Send", minimum_y=140))
        def repeater_complete():
            with sqlite3.connect(data/"inspector.sqlite") as db:
                return [json.loads(row[0]) for row in db.execute("SELECT data FROM repeater_results")
                    if json.loads(row[0])["state"] == "complete"]
        replay = wait_for(repeater_complete)
        assert replay[0]["source"] == "repeater_desktop"
        assert {row["id"]: row for row in captured()} == original_rows, "Repeater changed original traffic"
        navigate(desktop_window, "Preferences")
        click(desktop_window,310,150)
        screenshot(desktop_window,"preferences.png")
        navigate(desktop_window, "Drift")
        time.sleep(0.5)
        screenshot(desktop_window,"drift.png")
        if native:
            navigate(desktop_window, "Intercept")
            screenshot(desktop_window,"intercept-optional.png")
        with sqlite3.connect(data/"inspector.sqlite") as db:
            bodies=db.execute("SELECT COUNT(*) FROM bodies").fetchone()[0]
            drafts=db.execute("SELECT COUNT(*) FROM drafts").fetchone()[0]
        assert bodies>0,"No captured body persisted"
        assert drafts>0,"Repeater request was not saved"
        original_session=rows[0]["sessionId"]
        earlier_sessions={row["sessionId"] for row in captured()}
        def wait_for_restart():
            last_request=0
            def reconnected_capture():
                nonlocal last_request
                restarted=[row for row in (captured() or []) if row["source"]!="repeater_desktop" and row["sessionId"] not in earlier_sessions]
                if restarted:
                    return restarted
                if time.monotonic()-last_request>=1:
                    click(fixture_window,90,177)
                    last_request=time.monotonic()
                return None
            return wait_for(reconnected_capture,seconds=30)
        if observe_check:
            scans_before=discovery_scans()
            # Cover more than two old scan intervals while interacting with UI.
            for _ in range(6):
                navigate(desktop_window, "Drift")
                navigate(desktop_window, "Network")
                time.sleep(1.5)
            assert discovery_scans()==scans_before,"Full scans continued while connected"
            click(desktop_window,535,15) # Explicit Refresh remains available.
            wait_for(lambda: discovery_scans()==scans_before+1)
            time.sleep(3)
            fixture.stdin.write(b"R\n")
            fixture.stdin.flush()
            wait_for_restart()
            assert discovery_scans()>scans_before+1,"No fresh scan after hot restart"
            screenshot(desktop_window,"hot-restart-reconnected.png")
            earlier_sessions.update(row["sessionId"] for row in captured())
            (output/"scan-policy.json").write_text(json.dumps({"stable_automatic_scan_toolbar":True,"connected_scans_over_12_seconds":0,"manual_refresh":True,"hot_restart_reconnect":True,"slow_adb_seconds":2},indent=2))
        os.killpg(fixture.pid, signal.SIGTERM)
        fixture.wait(timeout=5)
        fixture=launch_fixture()
        fixture_window=window(fixture, fixture=True)
        restarted=wait_for_restart()
        screenshot(desktop_window,"auto-observe-reconnected.png")
        rows=captured()
        (output/"result.json").write_text(json.dumps({"automatic_discovery":True,"automatic_reconnect":True,"captured_exchanges":len(rows),"persisted_bodies":bodies,"saved_drafts":drafts,"session":original_session,"restarted_session":restarted[0]["sessionId"]},indent=2)+"\n")
        print("Real Flutter → VM → native UI capture passed:",len(rows),"exchange(s),",bodies,"body file(s)")
        if observe_check:
            subprocess.run(["xdotool","windowfocus",desktop_window,"key","ctrl+q"],check=True)
            desktop.wait(timeout=8)
    except Exception:
        if desktop is not None and desktop.poll() is None:
            screenshot(desktop_window,"failure.png")
        raise
    finally:
        for process in [desktop,fixture]:
            if process is not None and process.poll() is None:
                if process is fixture:
                    os.killpg(process.pid, signal.SIGTERM)
                else:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        fixture_log.close()
        desktop_log.close()
