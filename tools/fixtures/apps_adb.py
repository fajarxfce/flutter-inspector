#!/usr/bin/env python3
"""Isolated Android package-manager fixture; never invokes real ADB."""
import json
from pathlib import Path
import shlex
import sys
import time

root = Path(__file__).absolute().parent
state_file = root / "apps-state.json"
state = json.loads(state_file.read_text())
args = sys.argv[1:]
if args == ["devices", "-l"]:
    print("List of devices attached\nemulator-apps device model:Apps_Test\nemulator-other device model:Other_Device")
    sys.exit()
if args == ["forward", "--list"]:
    sys.exit()
if len(args) < 3 or args[0] != "-s" or args[2] != "shell":
    sys.exit()
serial = args[1]
cmd = shlex.split(" ".join(args[3:]))
with (root / "apps-calls.jsonl").open("a") as output:
    output.write(json.dumps([serial, *cmd]) + "\n")
mode = state.get("mode", "normal")
if mode == "hang":
    (root / "hanging").write_text(str(__import__('os').getpid()))
    time.sleep(20)

def save():
    state_file.write_text(json.dumps(state))

def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)

def user_arg():
    return int(cmd[cmd.index("--user")+1]) if "--user" in cmd else 0

def selected(package, user):
    return state.setdefault("apps", {}).setdefault(f"{serial}:{user}:{package}", dict(camera=False, cache=42, data=99, stopped=False))

if cmd == ["am", "get-current-user"]:
    print(10 if serial == "emulator-other" else 0)
elif cmd == ["pm", "help"]:
    print("Package manager commands:\n clear [--user USER_ID] " + ("[--cache-only] " if mode != "legacy" else "") + "PACKAGE")
elif cmd[:3] == ["pm", "list", "packages"]:
    if mode == "large":
        print("x" * (2*1024*1024+1))
    elif "-s" in cmd:
        print("package:android\npackage:com.android.settings")
    else:
        print("package:dev.example.alpha\npackage:dev.example.beta")
elif cmd[:3] == ["pm", "list", "permissions"]:
    for name,label,level in [("CAMERA","Camera access","dangerous"),("POST_NOTIFICATIONS","Show notifications","dangerous|instant"),("INTERNET","Network access","normal")]:
        print(f"+ permission:android.permission.{name}\n  label:{label}\n  protectionLevel:{level}")
elif cmd[:2] == ["dumpsys", "package"]:
    if mode == "readback" and state.get("changed"):
        fail("Device disconnected during readback")
    package = cmd[2]
    if package not in ["dev.example.alpha", "dev.example.beta", "android", "com.android.settings"]:
        print("Unable to find package:", package)
    else:
        system = package in ["android", "com.android.settings"]
        print(f"Packages:\n  Package [{package}] (abc):\n    versionCode=1 minSdk=26 targetSdk=36\n    versionName=1.0\n    pkgFlags=[ HAS_CODE {'SYSTEM' if system else 'DEBUGGABLE'} ]\n    requested permissions:\n      android.permission.CAMERA\n      android.permission.POST_NOTIFICATIONS\n      android.permission.INTERNET\n    install permissions:\n      android.permission.INTERNET: granted=true")
        for user in [0,10]:
            app = selected(package,user)
            grant = str(app["camera"]).lower()
            if mode == "different_users" and user == 10:
                grant = "true"
            flags = "SYSTEM_FIXED" if system else "USER_SET"
            print(f"    User {user}: installed=true stopped={str(app['stopped']).lower()} enabled=0\n      runtime permissions:\n        android.permission.CAMERA: granted={grant}, flags=[ {flags} ]\n        android.permission.POST_NOTIFICATIONS: granted=false, flags=[ POLICY_FIXED ]")
        print("Queries:\n  User 0:\n    runtime permissions:\n      android.permission.CAMERA: granted=true, flags=[]")
elif cmd[:2] in [["pm","grant"],["pm","revoke"]]:
    if mode == "reject":
        fail("SecurityException: denied by device policy")
    app = selected(cmd[-2],user_arg())
    if mode != "noop":
        app["camera"] = cmd[1] == "grant"
    state["changed"] = True
    save()
elif cmd[:2] == ["pm","clear"]:
    if mode == "reject":
        print("Failed")
    else:
        app = selected(cmd[-1],user_arg())
        app["cache"] = 0
        if "--cache-only" not in cmd:
            app.update(data=0, camera=False, stopped=True)
        state["changed"] = True
        save()
        print("Success")
elif cmd[:2] == ["am","force-stop"]:
    app = selected(cmd[-1],user_arg())
    app["stopped"] = True
    save()
elif cmd[:3] == ["cmd","package","resolve-activity"]:
    print(cmd[-1] + "/.MainActivity")
elif cmd[:2] == ["am","start"]:
    if "-n" in cmd:
        app = selected(cmd[-1].split('/')[0],user_arg())
        app["stopped"] = False
        save()
    print("Starting: Intent")
else:
    fail("Unsupported fixture command: " + repr(cmd))
