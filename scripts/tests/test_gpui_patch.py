"""GPUI provenance must tolerate Finder metadata while detecting source changes."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class GpuiPatchTests(unittest.TestCase):
    def test_finder_metadata_and_source_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            for name in ("Cargo.toml", "Cargo.lock", "scripts/check-gpui-patch.py"):
                shutil.copyfile(ROOT / name, root / name)
            vendor = root / "third_party/gpui"
            shutil.copytree(ROOT / "third_party/gpui", vendor,
                            ignore=shutil.ignore_patterns(".DS_Store"))

            def check():
                return subprocess.run([sys.executable, str(root / "scripts/check-gpui-patch.py")],
                                      capture_output=True, text=True, check=False)

            baseline = check()
            self.assertEqual(baseline.returncode, 0, baseline.stderr)
            # Same files and locations observed in the user's failing checkout.
            for name in (".DS_Store", "src/.DS_Store"):
                (vendor / name).write_bytes(b"Finder metadata")
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

            link = vendor / "src/.DS_Store"
            link.unlink()
            for target in ("interactive.rs", "missing-finder-target"):
                with self.subTest(symlink=target):
                    link.symlink_to(target)
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("unexpected files", result.stderr)
                    link.unlink()

            for name in ("src/interactive.rs", "src/accessibility.rs"):
                with self.subTest(changed=name):
                    path = vendor / name
                    original = path.read_bytes()
                    path.write_bytes(original + b"\n// changed source\n")
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("source/provenance mismatch", result.stderr)
                    path.write_bytes(original)

            missing = vendor / "src/interactive.rs"
            original = missing.read_bytes()
            missing.unlink()
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("source/provenance mismatch", result.stderr)
            missing.write_bytes(original)
            self.assertEqual(check().returncode, 0)


if __name__ == "__main__":
    unittest.main()
