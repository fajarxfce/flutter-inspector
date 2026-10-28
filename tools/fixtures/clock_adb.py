#!/usr/bin/env python3
"""Deterministic fake ADB for clock integration/UI tests; no device is contacted."""
import json
from pathlib import Path
import sys
import time

root = Path(__file__).parent
args = sys.argv[1:]
with (root / "calls.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\n")
if args == ["devices", "-l"]:
    print("List of devices attached\nemulator-clock device model:Clock_Test\nemulator-other device model:Other_Device")
    sys.exit(0)
if args == ["forward", "--list"]:
    sys.exit(0)
if "logcat" in args:
    while True:
        time.sleep(1)
assert args[:1] == ["-s"] and args[2:3] == ["shell"], args
serial = args[1]
command = args[3:]
state_file = root / ("other.json" if serial == "emulator-other" else "state.json")
state = json.loads(state_file.read_text())
if (root / "hang").exists() and command == ["date", "+%s"]:
    (root / "hung.pid").write_text(str(__import__("os").getpid()))
    time.sleep(30)


def save():
    state_file.write_text(json.dumps(state))


def change(kind):
    if state.get("reject_next") == kind:
        state.pop("reject_next")
        save()
        print("SecurityException: permission denied by test device", file=sys.stderr)
        sys.exit(1)
    if state.get("noop") == kind:
        print("Suggestion injected")
        sys.exit(0)


if command == ["date", "+%s"]:
    print((state["epoch_millis"] + int((time.monotonic() - state.get("at", time.monotonic())) * 1000)) // 1000)
elif command == ["getprop", "persist.sys.timezone"]:
    print(state["timezone"])
elif command[:3] == ["settings", "get", "global"]:
    print(int(state[command[3]]))
elif command == ["cat", "/proc/uptime"]:
    print("1000.00 1000.00")
elif command == ["am", "start", "-a", "android.settings.DATE_SETTINGS"]:
    print("Starting Date & time settings")
elif command[-1] == "help":
    mode = state.get("mode", "modern")
    if mode == "unsupported" or (mode == "legacy" and command[1] != "alarm") or (mode == "detector_only" and command[1] == "alarm"):
        print("Unknown service", file=sys.stderr)
        sys.exit(1)
    if command[1] == "alarm":
        print("Alarm commands:\nset-time TIME\nset-timezone TZ")
    else:
        print("Detector commands:\nset_auto_detection_enabled true|false")
        print("suggest_manual_time_zone --zone_id" if command[1] == "time_zone_detector" else "suggest_manual_time --elapsed_realtime --unix_epoch_time")
    sys.exit(255)
elif command[:3] == ["settings", "put", "global"] or command[2:3] == ["set_auto_detection_enabled"]:
    setting = command[3] if command[0] == "settings" else ("auto_time_zone" if command[1] == "time_zone_detector" else "auto_time")
    change(setting)
    state[setting] = command[-1] in ("1", "true")
    save()
elif command[2] in ("set-timezone", "suggest_manual_time_zone"):
    assert not state["auto_time_zone"], "Must disable automatic timezone first"
    change("timezone")
    state["timezone"] = command[-1]
    save()
elif command[2] in ("set-time", "suggest_manual_time"):
    assert not state["auto_time"], "Must disable automatic time first"
    change("time")
    state["epoch_millis"] = int(command[-1])
    state["at"] = time.monotonic()
    save()
else:
    print("Unsupported test command: " + repr(command), file=sys.stderr)
    sys.exit(1)
