"""Native app-manager regression with isolated fake ADB; no phone is modified."""
import json
import csv
import io
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from ui_driver import click, click_text, key, locate, navigate, screenshot, wait_for, window

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1] if len(sys.argv)>1 else root/'target/debug/flutter-inspector').resolve()
out = Path(os.environ.get('INSPECTOR_UI_OUTPUT', root/'artifacts/android-apps-ui'))
out.mkdir(parents=True,exist_ok=True)
(out/'failure.png').unlink(missing_ok=True)
probe = out/'probe.png'
with tempfile.TemporaryDirectory(prefix='inspector-apps-ui-') as temporary:
    data=Path(temporary)
    adb=data/'adb'
    shutil.copyfile(root/'tools/fixtures/apps_adb.py',adb)
    adb.chmod(0o755)
    state_file=data/'apps-state.json'
    state_file.write_text(json.dumps({'mode':'normal'}))
    with sqlite3.connect(data/'inspector.sqlite') as db:
        db.execute('CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL)')
        db.execute('INSERT INTO settings VALUES(1,?)',(json.dumps(dict(autoObserve=False,dark=True,retentionDays=30,quotaBytes=5*1024**3)),))
    env=dict(os.environ,INSPECTOR_DATA_DIR=str(data),INSPECTOR_ADB=str(adb),XDG_CONFIG_HOME=str(data/'config'))
    for name in ['WAYLAND_DISPLAY','WAYLAND_SOCKET','INSPECTOR_VM_URI']:
        env.pop(name,None)
    def state(): return json.loads(state_file.read_text())
    def app(serial='emulator-apps',user=0): return state().get('apps',{}).get(f'{serial}:{user}:dev.example.alpha',{})
    def ready(message='App details loaded'): wait_for(lambda:locate(win,probe,message))
    def mode(value):
        current=state();current['mode']=value;state_file.write_text(json.dumps(current))
    def modal_button(label):
        def point():
            screenshot(win,probe)
            enlarged=out/'modal-ocr.png'
            subprocess.run([shutil.which('magick') or 'convert',str(probe),'-crop','600x400+420+300','-resize','200%',str(enlarged)],check=True)
            result=subprocess.run(['tesseract',str(enlarged),'stdout','--psm','11','-c','tessedit_create_tsv=1'],capture_output=True,text=True,check=True)
            for word in csv.DictReader(io.StringIO(result.stdout),delimiter='\t',quoting=csv.QUOTE_NONE):
                if word['text'].lower()==label.lower():
                    return (420+(int(word['left'])+int(word['width'])//2)//2,300+(int(word['top'])+int(word['height'])//2)//2)
        click(win,*wait_for(point))
    def confirm(): modal_button('Confirm')
    with (out/'desktop.log').open('w') as log:
        process=subprocess.Popen([str(binary)],env=env,stdout=log,stderr=log)
        win=None
        try:
            win=wait_for(lambda:window(pid=process.pid))
            subprocess.run(['xdotool','windowsize',win,'1440','1000'],check=True)
            navigate(win,'Apps')
            wait_for(lambda:locate(win,probe,'dev.example.alpha'))
            assert state()=={'mode':'normal'},'Listing apps must not write anything'
            screenshot(win,out/'list.png')
            click_text(win,probe,'dev.example.alpha')
            ready()
            screenshot(win,out/'permissions.png')
            click_text(win,probe,'Denied')
            wait_for(lambda:app().get('camera') is True)
            ready('Permission updated')
            click_text(win,probe,'dev.example.beta')
            ready()
            assert locate(win,probe,'Denied')
            click_text(win,probe,'System')
            wait_for(lambda:locate(win,probe,'com.android'))
            assert not locate(win,probe,'Version 1.0'),'Filter switch must clear the hidden app detail'
            click_text(win,probe,'com.android')
            ready()
            assert locate(win,probe,'locked')
            screenshot(win,out/'system.png')
            click_text(win,probe,'Installed')
            click_text(win,probe,'dev.example.alpha')
            ready()
            click_text(win,probe,'Storage')
            click_text(win,probe,'Clear cache')
            screenshot(win,out/'cache-confirmation.png')
            confirm()
            wait_for(lambda:app().get('cache')==0)
            ready('App details refreshed')
            assert app()['data']==99
            click_text(win,probe,'Clear storage')
            modal_button('Cancel')
            assert app()['data']==99,'Cancelling storage clear must preserve data'
            click_text(win,probe,'Clear storage')
            confirm()
            wait_for(lambda:app().get('data')==0)
            ready('App details refreshed')
            screenshot(win,out/'storage-cleared.png')
            click_text(win,probe,'App actions')
            click_text(win,probe,'Open app')
            wait_for(lambda:app().get('stopped') is False)
            ready('App opened')
            click_text(win,probe,'Force stop')
            confirm()
            wait_for(lambda:app().get('stopped') is True)
            ready('App force-stopped')
            mode('hang')
            click_text(win,probe,'Refresh details')
            navigate(win,'Network')
            wait_for(lambda:locate(win,probe,'Waiting for traffic'),seconds=3)
            navigate(win,'Apps')
            ready('ADB timed out')
            mode('normal')
            click_text(win,probe,'Refresh details')
            ready()
            click_text(win,probe,'Apps Test')
            click_text(win,probe,'Other Device')
            wait_for(lambda:locate(win,probe,'Android user 10'))
            assert not locate(win,probe,'Version 1.0'),'Device switch must clear prior details'
            click_text(win,probe,'dev.example.alpha')
            ready()
            click_text(win,probe,'Permissions')
            click_text(win,probe,'Denied')
            wait_for(lambda:app('emulator-other',10).get('camera') is True)
            ready('Permission updated')
            assert app()['camera'] is False,'Permission action changed the previous device'
            screenshot(win,out/'other-device.png')
            key(win,'ctrl+q')
            process.wait(timeout=8)
            (out/'result.json').write_text(json.dumps(dict(checks=['passive_list','system_filter','permission_grant','selection_reset','cache_preserves_data','cancel_storage','clear_storage','launch_force_stop','timeout_navigation_recovery','device_user_isolation']),indent=2))
            print('Android app manager UI passed')
        finally:
            if process.poll() is None:
                if win:
                    screenshot(win,out/'failure.png')
                    key(win,'ctrl+q')
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill();process.wait()
