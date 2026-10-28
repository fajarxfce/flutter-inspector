"""Publish complete native packages to the repository's rolling preview release.

Only the preview job grants write access. --prepare-only checks local packages
and writes checksums/notes without contacting GitHub.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

TAG = "preview"


def prepare(directory, version, repository, commit):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Invalid GitHub repository")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("Expected the full build commit SHA")
    suffixes = ["linux-x86_64.tar.gz", "windows-amd64.zip", "darwin-arm64.zip",
                "darwin-arm64.dmg", "darwin-x86_64.zip", "darwin-x86_64.dmg"]
    names = {f"flutter-inspector-{version}-{suffix}" for suffix in suffixes}
    actual = {p.name for p in directory.iterdir() if p.is_file()} - {"SHA256SUMS", "release-notes.md"}
    if actual != names:
        raise ValueError(f"Incomplete or unexpected release packages. Missing: {sorted(names-actual)}; unexpected: {sorted(actual-names)}")
    files = [directory / name for name in sorted(names)]
    if any(p.stat().st_size == 0 for p in files):
        raise ValueError("Release packages must not be empty")
    checksums = directory / "SHA256SUMS"
    lines = []
    for path in files:
        with path.open("rb") as source:
            lines.append(f"{hashlib.file_digest(source, 'sha256').hexdigest()}  {path.name}\n")
    checksums.write_text("".join(lines))
    notes = directory / "release-notes.md"
    notes.write_text(f"""Latest development build from [`{commit[:7]}`](https://github.com/{repository}/commit/{commit}).

| Platform | Download |
| --- | --- |
| Windows x64 | `windows-amd64.zip` |
| macOS Apple Silicon | `darwin-arm64.dmg` or `.zip` |
| macOS Intel | `darwin-x86_64.dmg` or `.zip` |
| Linux x64 | `linux-x86_64.tar.gz` |

This prerelease is updated after every successful build of `main`. Packages are unsigned. Verify downloads with `SHA256SUMS`.
""")
    return files + [checksums], notes


def api(repository, endpoint, method="GET", payload=None, optional=False):
    args = ["gh", "api", "--method", method, f"repos/{repository}/{endpoint}"]
    if payload is not None:
        args += ["--input", "-"]
    result = subprocess.run(args, input=json.dumps(payload) if payload is not None else None,
                            capture_output=True, text=True)
    if result.returncode:
        if optional and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout) if result.stdout.strip() else None


def publish(repository, commit, files, notes):
    # Older builds may finish after a newer push. They must not replace preview.
    if api(repository, "commits/main")["sha"] != commit:
        print("A newer main commit exists; this build will not replace preview.")
        return
    release = api(repository, f"releases/tags/{TAG}", optional=True)
    values = dict(name="Latest preview", body=notes.read_text(), draft=True, prerelease=True)
    if release:
        release = api(repository, f"releases/{release['id']}", "PATCH", values)
    ref = api(repository, f"git/ref/tags/{TAG}", optional=True)
    if ref:
        api(repository, f"git/refs/tags/{TAG}", "PATCH", dict(sha=commit, force=True))
    else:
        api(repository, "git/refs", "POST", dict(ref=f"refs/tags/{TAG}", sha=commit))
    if not release:
        release = api(repository, "releases", "POST", dict(values, tag_name=TAG, target_commitish=commit))
    subprocess.run(["gh", "release", "upload", TAG, "--repo", repository,
                    "--clobber", *[str(p) for p in files]], check=True)
    assets = api(repository, f"releases/{release['id']}/assets")
    expected = {p.name: p.stat().st_size for p in files}
    uploaded = {asset["name"]: asset["size"] for asset in assets if asset["name"] in expected}
    if uploaded != expected:
        raise RuntimeError("Uploaded assets do not match all verified build packages; release remains draft")
    # A version bump changes filenames. Remove only obsolete rolling-preview assets.
    for asset in assets:
        if asset["name"] not in expected:
            api(repository, f"releases/assets/{asset['id']}", "DELETE")
    result = api(repository, f"releases/{release['id']}", "PATCH",
                 dict(draft=False, prerelease=True, make_latest="false"))
    print("Published", result["html_url"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    repository = os.environ["GITHUB_REPOSITORY"]
    commit = os.environ["GITHUB_SHA"]
    files, notes = prepare(args.assets, version, repository, commit)
    if args.prepare_only:
        print("Verified", len(files)-1, "native packages and generated SHA256SUMS")
    else:
        publish(repository, commit, files, notes)


if __name__ == "__main__":
    main()
