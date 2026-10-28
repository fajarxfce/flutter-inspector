"""Small X11 driver for isolated native UI integration checks."""
import csv
import io
import subprocess
import time


def wait_for(callback, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = callback()
        if value:
            return value
        time.sleep(0.15)
    raise RuntimeError("Timed out: " + callback.__name__)


def window(pid=None, name=None):
    result = subprocess.run(["xdotool", "search", "--onlyvisible", *( ["--pid", str(pid)] if pid else ["--name", name])], capture_output=True, text=True)
    return result.stdout.splitlines()[0] if result.returncode == 0 else None


def click(win, x, y):
    subprocess.run(["xdotool", "windowraise", win, "windowfocus", win, "mousemove", "--window", win, str(x), str(y), "click", "1"], check=True)
    time.sleep(0.15)


def key(win, keys):
    subprocess.run(["xdotool", "windowfocus", win, "key", "--clearmodifiers", keys], check=True)


def navigate(win, tab):
    """Default-scale sidebar coordinates shared by the non-OCR smoke drivers."""
    tabs = ("Network", "Repeater", "Intercept", "Preferences", "Drift", "Projects", "Settings", "GPS", "Utilities")
    click(win, 80, 154 + 42 * tabs.index(tab))


def enter(win, x, y, text):
    click(win, x, y)
    key(win, "ctrl+a")
    for i, line in enumerate(text.split("\n")):
        if i:
            key(win, "Return")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "1", "--", line], check=True)
    time.sleep(0.15)


def screenshot(win, path):
    subprocess.run(["xdotool", "windowraise", win], check=True)
    time.sleep(0.2)
    subprocess.run(["import", "-window", win, str(path)], check=True)


def words(win, path):
    screenshot(win, path)
    result = subprocess.run(["tesseract", str(path), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1"], capture_output=True, text=True, check=True)
    # Tesseract TSV contains literal quote characters, not CSV-escaped fields.
    # A recognized `"` must not swallow the following rows and their coordinates.
    return [w for w in csv.DictReader(io.StringIO(result.stdout), delimiter="\t", quoting=csv.QUOTE_NONE) if w["text"].strip()]


def locate(win, path, phrase, minimum_y=0):
    items = [w for w in words(win, path) if int(w["top"]) >= minimum_y]
    wanted = phrase.lower().split()
    for i in range(len(items) - len(wanted) + 1):
        chunk = items[i:i + len(wanted)]
        if all(token in item["text"].lower().replace("logeat", "logcat") for token, item in zip(wanted, chunk)):
            # Prevent an accidental match across unrelated rows.
            if max(int(w["top"]) for w in chunk) - min(int(w["top"]) for w in chunk) > 12:
                continue
            return (int(chunk[0]["left"]) + int(chunk[0]["width"]) // 2,
                    int(chunk[0]["top"]) + int(chunk[0]["height"]) // 2)
    # Native small buttons sometimes lose the space in OCR (Clearitem/Clearall).
    joined = "".join(wanted)
    for word in items:
        if joined == word["text"].lower().strip("‘’"):
            return (int(word["left"]) + int(word["width"]) // 2,
                    int(word["top"]) + int(word["height"]) // 2)
    return None


def click_text(win, path, phrase, seconds=15):
    point = wait_for(lambda: locate(win, path, phrase), seconds)
    click(win, *point)
    return point
