"""Real desktop -> ADB -> Android -> geolocator smoke. Isolated emulator only.

Install examples/gps_fixture first, then run under Xvfb:
python3 tools/gps_real_ui.py emulator-5580 [binary]
"""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from ui_driver import click_text, key, locate, navigate, screenshot, wait_for, window

root = Path(__file__).resolve().parents[1]
serial = sys.argv[1]
assert serial.startswith("emulator-"), "Use an isolated test emulator"
binary = Path(sys.argv[2]).resolve() if len(sys.argv)>2 else root / "target/release/flutter-inspector"
adb = shutil.which('adb')
def shell(*args):
    return subprocess.check_output([adb,'-s',serial,'shell',*args],text=True,timeout=10).strip()

shell('am','force-stop','dev.flutterinspector.gps_fixture')
shell('am','start','-n','dev.flutterinspector.gps_fixture/.MainActivity')
time.sleep(3)
pid = shell('pidof','dev.flutterinspector.gps_fixture')
assert pid
out = root / 'artifacts/gps-real-ui'
out.mkdir(parents=True,exist_ok=True)
probe = out / 'probe.png'
def fixes():
    logs = subprocess.check_output([adb,'-s',serial,'logcat','-d','--pid='+pid],text=True,timeout=8)
    return [json.loads(line.split('INSPECTOR_GPS_FIX ',1)[1]) for line in logs.splitlines() if 'INSPECTOR_GPS_FIX ' in line]

with tempfile.TemporaryDirectory(prefix='inspector-gps-real-') as temporary:
    data = Path(temporary)
    proxy = data / 'adb'
    # Make it impossible for this test instance to reach the user's phone.
    proxy.write_text(f'''#!{sys.executable}
import os,sys
args=sys.argv[1:]
if args==['devices','-l']:
 print('List of devices attached\\n{serial} device model:GPS_Fixture')
elif args[:2]==['-s','{serial}']:
 os.execv({adb!r},[{adb!r},*args])
''')
    proxy.chmod(0o755)
    with sqlite3.connect(data/'inspector.sqlite') as db:
        db.execute('CREATE TABLE settings(id INTEGER PRIMARY KEY,data TEXT NOT NULL)')
        db.execute('INSERT INTO settings VALUES(1,?)',(json.dumps(dict(autoObserve=False,dark=True,retentionDays=30,quotaBytes=5*1024**3)),))
    env = dict(os.environ,INSPECTOR_DATA_DIR=str(data),INSPECTOR_ADB=str(proxy),INSPECTOR_FRAME_LOG=str(out/'frames.json'))
    for name in ['WAYLAND_DISPLAY','WAYLAND_SOCKET','INSPECTOR_VM_URI']:env.pop(name,None)
    with (out/'desktop.log').open('w') as log:
        desktop = subprocess.Popen([str(binary)],env=env,stdout=log,stderr=log)
        win = None
        try:
            win = wait_for(lambda:window(pid=desktop.pid))
            navigate(win,'GPS')
            wait_for(lambda:locate(win,probe,'Start GPS'))
            click_text(win,probe,'Start GPS')
            wait_for(lambda:any(p['mock'] and p['speed']==0 and abs(p['altitude']-10)<.01 for p in fixes()))
            click_text(win,probe,'Drive / update direction')
            def moved():
                moving=[p for p in fixes() if p['mock'] and abs(p['speed']-30/3.6)<.01 and abs(p['heading'])<.01]
                return len(moving)>2 and moving[-1]['latitude']>moving[0]['latitude']
            wait_for(moved,seconds=20)
            screenshot(win,out/'geolocator-driving.png')
            click_text(win,probe,'Stop & restore')
            invoke='CLASSPATH=/data/local/tmp/flutter-inspector-gps-v1.jar app_process /system/bin dev.flutterinspector.gps.Main --probe'
            wait_for(lambda:not json.loads(shell(invoke))['recoveryNeeded'])
            key(win,'ctrl+q')
            desktop.wait(timeout=8)
            (out/'result.json').write_text(json.dumps(dict(serial=serial,geolocator='14.1.1',android_sdk=shell('getprop','ro.build.version.sdk'),fix_count=len(fixes()),verified=['desktop start','heading drive','fused stream movement','speed','bearing','altitude','mock flag','stop restore']),indent=2))
            print('PASS: real desktop -> ADB -> Android mock providers -> Geolocator fused stream, including Stop restoration')
        finally:
            if desktop.poll() is None:
                if win:key(win,'ctrl+q')
                try:desktop.wait(timeout=8)
                except subprocess.TimeoutExpired:desktop.kill()
