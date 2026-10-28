from pathlib import Path
import tempfile
import unittest
from publish_preview import prepare


class ReleasePackagesTest(unittest.TestCase):
    def test_partial_build_cannot_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "flutter-inspector-0.1.0-linux-x86_64.tar.gz").write_bytes(b"fixture")
            with self.assertRaisesRegex(ValueError, "Missing"):
                prepare(root, "0.1.0", "owner/repo", "a"*40)

    def test_all_architectures_receive_checksums_and_reruns_are_valid(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for suffix in ["linux-x86_64.tar.gz", "windows-amd64.zip", "darwin-arm64.zip",
                           "darwin-arm64.dmg", "darwin-x86_64.zip", "darwin-x86_64.dmg"]:
                (root / f"flutter-inspector-0.1.0-{suffix}").write_bytes(b"fixture")
            files, notes = prepare(root, "0.1.0", "owner/repo", "a"*40)
            self.assertEqual(len(files), 7)
            self.assertEqual(len((root / "SHA256SUMS").read_text().splitlines()), 6)
            self.assertIn("owner/repo/commit/" + "a"*40, notes.read_text())
            prepare(root, "0.1.0", "owner/repo", "a"*40)
            (root / "stale-build.zip").write_bytes(b"fixture")
            with self.assertRaisesRegex(ValueError, "unexpected"):
                prepare(root, "0.1.0", "owner/repo", "a"*40)


if __name__ == "__main__":
    unittest.main()
