"""Isolated GPS UI/actor/protocol smoke test. Uses fake ADB, never a connected device.

xvfb-run -a -s '-screen 0 1600x1000x24' python3 tools/gps_ui.py [binary]
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import click, click_text, key, locate, navigate, screenshot, wait_for, window

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/debug/flutter-inspector"
out = root / "artifacts/gps-ui"
out.mkdir(parents=True, exist_ok=True)
probe = out / "probe.png"
with tempfile.TemporaryDirectory(prefix="inspector-gps-ui-") as temporary:
    data = Path(temporary)
    adb = data / "adb"
    calls = data / "fixes.jsonl"
    adb.write_text('''#!/usr/bin/env python3
import sys,json,os,time
args=sys.argv[1:]
if args==['devices','-l']:
 print('List of devices attached\\nemulator-test device product:gps model:GPS_Test device:gps')
elif 'push' in args:
 pass
elif args and 'app_process' in args[-1]:
 if '--restore' in args[-1]: print('{"ok":true,"stopped":true}',flush=True)
 else:
  print('{"ready":true,"protocol":1}',flush=True)
  for line in sys.stdin:
   request=json.loads(line)
   with open(os.environ['GPS_TEST_CALLS'],'a') as log: log.write(json.dumps(request)+'\\n')
   if request['op']=='fix' and os.path.exists(os.environ['GPS_TEST_HANG']):
    time.sleep(20)
   if request['op']=='stop':
    print('{"ok":true,"stopped":true}',flush=True); break
   print(json.dumps({'ok':True,'fix':request['fix']}),flush=True)
''')
    adb.chmod(0o755)
    with sqlite3.connect(data / "inspector.sqlite") as db:
        db.execute("CREATE TABLE settings(id INTEGER PRIMARY KEY,data TEXT NOT NULL)")
        db.execute("INSERT INTO settings VALUES(1,?)",(json.dumps(dict(autoObserve=False,dark=True,retentionDays=30,quotaBytes=5*1024**3)),))
    hang = data / 'hang'
    env = dict(os.environ, INSPECTOR_DATA_DIR=str(data), INSPECTOR_ADB=str(adb), GPS_TEST_CALLS=str(calls), GPS_TEST_HANG=str(hang))
    env.pop("WAYLAND_DISPLAY",None)
    env.pop("WAYLAND_SOCKET",None)
    env.pop("INSPECTOR_VM_URI",None)
    def commands():
        return [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []
    with (out / "desktop.log").open("w") as log:
        desktop = subprocess.Popen([str(binary)],env=env,stdout=log,stderr=log)
        win = None
        try:
            win = wait_for(lambda: window(pid=desktop.pid))
            subprocess.run(['xdotool','windowsize',win,'1440','1000'],check=True)
            navigate(win,"GPS")
            wait_for(lambda: locate(win,probe,"Start GPS"))
            assert not commands(), "Opening GPS must not inject a position"
            # Build a route in the native map; arbitrary clicks produce valid coordinates.
            click_text(win,probe,"Add waypoints")
            click(win,880,360)
            click(win,980,410)
            click(win,1080,360)
            screenshot(win,out / "route.png")
            start_button = click_text(win,probe,"Start GPS")
            wait_for(lambda: len(commands()) >= 2)
            click_text(win,probe,"Play route")
            wait_for(lambda: any(c.get('fix',{}).get('speed',0)>0 for c in commands()))
            pause_button = click_text(win,probe,"Pause")
            wait_for(lambda: commands()[-1].get('fix',{}).get('speed')==0)
            at = commands()[-1]['fix']
            time.sleep(1.2)
            assert commands()[-1]['fix']==at, "Paused route moved"
            click(win,*pause_button)
            wait_for(lambda: commands()[-1].get('fix',{}).get('speed',0)>0)
            screenshot(win,out / "playing.png")
            navigate(win,"Network")
            count = len(commands())
            wait_for(lambda: len(commands()) > count + 2)
            navigate(win,"GPS")
            click_text(win,probe,"Stop & restore")
            wait_for(lambda: commands()[-1]['op']=='stop')
            count = len(commands())
            time.sleep(1.2)
            assert len(commands())==count, "Commands continued after Stop"
            # A hung helper must time out without blocking navigation or leaving Start disabled.
            hang.touch()
            click(win,*start_button)
            wait_for(lambda: len(commands()) > count)
            navigate(win,"Network")
            wait_for(lambda: locate(win,probe,"Waiting for traffic"),seconds=4)
            time.sleep(9)
            navigate(win,"GPS")
            hang.unlink()
            count = len(commands())
            click(win,*start_button)
            wait_for(lambda: len(commands()) > count)
            key(win,'ctrl+q')
            desktop.wait(timeout=8)
            assert commands()[-1]['op']=='stop', "Closing window did not restore GPS"
            print("PASS: map route, explicit start, play/pause/resume, background movement, stop, stalled-helper recovery, close cleanup")
        except Exception:
            if win: screenshot(win,out / "failure.png")
            raise
        finally:
            if desktop.poll() is None:
                desktop.terminate()
                try: desktop.wait(timeout=8)
                except subprocess.TimeoutExpired: desktop.kill()
