"""Finder metadata and source integrity for the pinned dependency patches."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DependencyPatchTests(unittest.TestCase):
    def test_finder_metadata_is_ignored_but_unexpected_files_and_links_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            for name in ("Cargo.toml", "Cargo.lock", "scripts/check-dependency-patches.py"):
                shutil.copyfile(ROOT / name, root / name)
            for crate in ("block", "proc-macro-error2"):
                shutil.copytree(ROOT / "third_party" / crate, root / "third_party" / crate,
                                ignore=shutil.ignore_patterns(".DS_Store"))
            vendor = root / "third_party/proc-macro-error2"

            def check():
                return subprocess.run(
                    [sys.executable, str(root / "scripts/check-dependency-patches.py")],
                    capture_output=True, text=True, check=False)

            baseline = check()
            self.assertEqual(baseline.returncode, 0, baseline.stderr)
            # The file observed in the owner's checkout on 30 October 2026.
            (vendor / ".DS_Store").write_bytes(b"Finder metadata")
            metadata = check()
            self.assertEqual(metadata.returncode, 0, metadata.stderr)

            for name in ("src/unexpected.rs", "src/.DS_Store.rs"):
                with self.subTest(unexpected=name):
                    path = vendor / name
                    path.write_text("fn unexpected() {}\n")
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("unexpected files", result.stderr)
                    path.unlink()

            (vendor / ".DS_Store").unlink()
            link = vendor / "src/.DS_Store"
            link.symlink_to("lib.rs")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unexpected files", result.stderr)


if __name__ == "__main__":
    unittest.main()
