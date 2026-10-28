"""Regenerate native icons from packaging/icon.svg using rsvg-convert.

Generated files are checked in: ordinary builds need no graphics tooling.
ICO and ICNS use lossless PNG representations at native desktop sizes.
"""
from pathlib import Path
import struct
import subprocess

root = Path(__file__).resolve().parents[1]
assets = root / "packaging"
sizes = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
images = {
    size: subprocess.check_output([
        "rsvg-convert", "--width", str(size), "--height", str(size),
        str(assets / "icon.svg"),
    ]) for size in sizes
}
(assets / "icon.png").write_bytes(images[256])

ico_sizes = sizes[:7]
offset = 6 + 16 * len(ico_sizes)
entries = []
for size in ico_sizes:
    data = images[size]
    entries.append(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
    offset += len(data)
(assets / "icon.ico").write_bytes(
    struct.pack("<HHH", 0, 1, len(ico_sizes)) + b"".join(entries)
    + b"".join(images[size] for size in ico_sizes)
)

representations = {
    b"icp4": 16, b"icp5": 32, b"icp6": 64, b"ic07": 128,
    b"ic08": 256, b"ic09": 512, b"ic10": 1024,
    b"ic11": 32, b"ic12": 64, b"ic13": 256, b"ic14": 512,
}
chunks = b"".join(
    kind + struct.pack(">I", len(images[size]) + 8) + images[size]
    for kind, size in representations.items()
)
(assets / "icon.icns").write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
print("Generated PNG, ICO and ICNS from", assets / "icon.svg")
